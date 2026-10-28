# Spike: private `aida-core` (or carved-out proprietary crate) with public-repo CI

- Spike: SPIKE-b6b1278890774ef993d2ebc327862470-001
- Date: 2026-10-28
- Basis: `origin/main` at `4e961089c95`
- Type: research write-up; no code changes

## 1. Question

Can `aida-core`, or a proprietary crate carved out of it, live in a private
GitHub repository while `aida-cli`, `aida-cli-lib`, `aida-tui`, `aida-server`
and the other crates stay in the public `joemooney/aida` repository, with CI
still running on public GitHub Actions? If so, what does it cost and how should
it be done?

## 2. Facts measured in this workspace

### 2.1 The repository is already public, with full history

`gh repo view joemooney/aida` reports `visibility: PUBLIC` (created
2025-11-23, 1 fork, 5 stars). `git log -- aida-core` lists **966 commits**
touching `aida-core`, all of them already published. Every crate is
`license = "MIT"` (workspace `Cargo.toml`), and none declares `publish = false`.

This changes the spike's premise. "Git history scrubbing" can no longer keep
the existing code private. Whatever `aida-core` contains at `4e961089c95` is
already public under MIT, and anyone who cloned or forked it keeps that grant
permanently. A split only protects code written **after** the split.

### 2.2 Size

| Crate | Rust lines | Depends on `aida-core` |
|---|---:|---|
| `aida-cli-lib` | 801,480 | yes (features `postgres,gitlab,github,jira`) |
| `aida-core` | 143,944 | — |
| `aida-cli` | 35,102 | yes (dev-dependency; binary is a stub over `aida-cli-lib`) |
| `aida-tui` | 28,875 | yes |
| `aida-server` | 14,778 | yes (feature `jira`, optional `postgres`) |
| `aida-generate-types` | 172 | yes |
| `aida-crate` (`aida`) | 26 | yes |

Every other workspace crate depends on `aida-core`. Nothing ships without it.

### 2.3 API surface actually consumed

`aida-core/src/lib.rs` has 99 `pub mod`/`pub use` lines. Counting distinct
`aida_core::<module>` paths referenced from the other crates (a lower bound,
because grouped `use aida_core::{…}` imports are undercounted):

| Consumer | Distinct modules | References | Files |
|---|---:|---:|---:|
| `aida-cli-lib` | 82 | 2,984 | 521 |
| `aida-server` | 13 | 43 | 8 |
| `aida-tui` | 11 | 54 | 12 |
| `aida-cli` | 8 | 61 | 15 |
| `aida-generate-types` | 3 | 5 | 1 |
| **Any consumer** | **85** | | |

The most-used modules in `aida-cli-lib`: `git_ops` (811 references),
`mailbox` (262), `models` (258), `db` (189), `scaffolding` (163), `liveness`
(147), `conflict` (100), `object_store` (85), `write_atomic` (63),
`mailbox_lifecycle` (59), `pickability` (58).

So `aida-core` is not a small kernel behind a narrow interface. It is the
lower half of one program, and the CLI calls into about 85 of its roughly 99
public modules. These include free functions, concrete structs and
low-level helpers, not traits.

### 2.4 Change coupling (last 200 merged PRs)

The 200 most recent first-parent commits on `main` (squash merges,
2026-10-18 → 2026-10-28):

| Measure | PRs | Share |
|---|---:|---:|
| Touched anything under `aida-core/` | 69 | 35% |
| Touched `aida-core/src/` | 57 | 29% |
| Touched `aida-core/src/` **and** another Rust crate | **50** | **25%** |
| Touched only `aida-core/templates/` (no core `src`) | 12 | 6% |

Of the PRs that touched `aida-core/src/`, **50 of 57 (88%) also changed a
dependent crate in the same PR**. Most of these paired `aida-core` with
`aida-cli-lib` (41), followed by `aida-cli` + `aida-cli-lib` (14), and then
`aida-tui` or `aida-server`. Splitting the repository would make roughly one
PR in four (about 5 a day at current throughput) a coordinated two-repo
change.

### 2.5 Templates live inside `aida-core`

`aida-core/templates/` (2.4 MB, 245 files) is the source of truth for skills,
hooks, discipline docs and scaffolding. It is embedded in the binary and
copied into users' repositories, so it is public by nature. The production
embeds are in `aida-core` itself: five in `aida-core/src/scaffolding/hooks.rs`
and one in `aida-core/src/scaffolding/inventory.rs`. `aida-cli-lib` has 11
`include_str!`/`include_dir!` template sites, but only one is production code
(`src/cli.rs`); the other 10 are in tests. In all, 22 `aida-cli-lib/src` files
refer to the templates path, and so do `make sync-templates`,
`make check-templates` and the `.claude/` symlinks. Any split has to move templates out of the private
crate first.

### 2.6 CI layout

`.github/workflows/ci.yml` (854 lines) runs on `push`, `pull_request` and
`merge_group`. The matrix is `ubuntu-latest` (Linux only, ADR-72) × 7 shards
(`lib-0`, `lib-1`, `cli-0`, `cli-1`, `rest`, `gates`, `contracts`), with a
40-minute timeout. It runs `cargo build --workspace`, nextest shards,
`cargo clippy --workspace`, and the gate scripts that the local
`make check-ci-*` targets also run from the same step bodies. Other workflows
are `release.yml` (tag-triggered `cargo build --release` that uploads
binaries), `cross-platform.yml`, `merge-hold-gate.yml`, `web.yml`,
`weekly-linux.yml` and `test-link-timings.yml`. All CI today assumes a
single-checkout workspace with `path = "../aida-core"` dependencies.

### 2.7 External contribution

Of the last 200 merged PRs on GitHub, 0 came from a fork. Every contributor
today is the owner or an agent working in an owner-controlled branch.

## 3. The constraints in the spike, with mechanisms and costs

### 3.1 Public CI logs

**Risk:** Actions logs on a public repository are world-readable. rustc
errors print source spans, `cargo test` prints panics with file:line and
`assert_eq!` diffs, `RUST_BACKTRACE` prints frames, and clippy prints code
snippets. If the private crate is compiled or tested in public CI, fragments
of it appear in those logs.

**Mechanisms:**

1. **Build the private crate only in the private repository.** The private
   repo runs its own CI (unit tests, clippy) and publishes a versioned
   artifact. Public CI never compiles private source. Options for the artifact:
   - *Private Cargo registry* (Cloudsmith, Kellnr, Shipyard, or an Artifactory
     sparse registry): public CI uses `cargo` with a registry token, but
     `cargo build` still unpacks and compiles the source on the public runner,
     so errors still show it. **This does not solve log leakage.**
   - *Precompiled `rlib`/`staticlib` behind a C ABI or a stable Rust ABI
     shim:* Rust has no stable ABI, so the toolchain must be pinned exactly on
     both sides, generics and `impl Trait` cannot cross the boundary, and
     monomorphised code has to live on the private side. This is the only
     option that keeps private source off public runners entirely, and it is
     a large engineering cost (see §4).
2. **Integration tests via `repository_dispatch`.** Public CI builds against
   the public trait crate with a stub implementation. A dispatch to the
   private repo runs the full integration suite there and reports a commit
   status back with a fine-grained PAT or GitHub App. Cost: one more workflow
   in each repo, a GitHub App or PAT with `statuses:write` on the public repo,
   and a merge gate that waits for an external status. Latency is one more CI
   run (the full suite takes 17–25 minutes today).
3. **Log hygiene as defence in depth:** `RUST_BACKTRACE=0`, `--quiet`, and
   `::add-mask::`. These are not sufficient, because rustc diagnostics cannot
   be masked by content.

### 3.2 Fork PRs and secrets

**Risk:** `pull_request` runs from forks get no secrets, so they cannot fetch
private code. `pull_request_target` does get secrets, but it checks out
attacker-controlled code with them. That is the classic pwn-request pattern,
and here it would leak the private source through a malicious `build.rs`.

**Mechanisms:**

- Make the public workspace build and test with **no private code by
  default** (open-core, §4). Fork PRs then get full public CI with no secrets.
- Private-dependent jobs run only for same-repo branches
  (`if: github.event.pull_request.head.repo.full_name == github.repository`)
  or after a maintainer label in a separate `workflow_run`. That job checks
  out the merge commit, never runs fork-controlled `build.rs` with the token,
  and uses a read-only, single-repo fine-grained token with an expiry.
- **Cost today: close to zero**, because 0 of the last 200 PRs came from
  forks. The constraint is real only if outside contributors become a goal.

### 3.3 Coordinated cross-repo changes

**Risk:** 25% of PRs change `aida-core` and a consumer together (§2.4). The
AIDA workflow itself (worktree per lane, `aida ship`, merge queue, the
`(SPEC-ID)` trailer driving completion on merge, `aida dev build` smoke and
promote) assumes one repository and one commit.

**Mechanisms, from cheapest to most robust:**

1. **Version pin plus a two-step merge:** merge to the private repo, bump the
   pin (`aida-core = { git = …, rev = … }` or a registry version) in a public
   PR. Every coupled change becomes two PRs, two reviews and two CI runs, and
   the public `main` is red or stale in between. At about 5 coupled PRs a day
   this roughly doubles review and merge load for a quarter of the work.
2. **Branch-name matching:** public CI checks out the private repo at the same
   branch name if one exists, otherwise at `main`. Merge order is enforced by
   a bot (private first, then public with the pin updated automatically).
   Cost: custom orchestration in `aida ship`/merge-queue code, failure modes
   when one side merges and the other is rejected, and a second merge-hold
   gate.
3. **Private monorepo as the source of truth, public repo as a mirror:** all
   work stays in one private repository (today's workflow, unchanged). A
   `copybara`/`josh`-style export job pushes the public subset (everything
   except the private crate, with the private dependency rewritten to a stub
   or registry version) to `joemooney/aida`. Atomic changes are preserved
   internally. Public contributions have to be imported back through the
   exporter. Cost: an exporter and its rules (about 1–2 weeks to set up, then
   ongoing upkeep), public history becomes synthetic, and public CI tests the
   exported tree rather than what is actually developed.

Option 3 is the only one that does not slow the current agent-driven
throughput. Options 1 and 2 put a coordination tax on 25% of PRs.

### 3.4 Git history scrubbing

**Risk as written in the spike:** "making the existing repo public requires
`git filter-repo`." The repository is already public (§2.1), so this risk is
inverted: nothing already pushed can be withdrawn.

**What remains possible:**

- `git filter-repo --path aida-core/src --invert-paths` on a **new** public
  repository whose history starts at the split. All public SHAs change, which
  breaks every `trace:` commit reference, PR link, the `aida-store` orphan
  branch's commit pointers, and the existing fork. The old repository would
  have to be deleted or archived, and its 4,045 commits are still MIT-licensed
  wherever they were cloned.
- A clean-snapshot push (`git checkout --orphan`) has the same effects and is
  simpler.
- Neither undoes the license. The only thing either one achieves is that
  **future** private code never enters public history.

**Cost:** high, and it protects nothing that already exists. Not recommended.

## 4. What an open-core trait boundary would need to cover

For "public crates build from source with no private code" (option 2 in the
spike, and the prerequisite for §3.1 and §3.2), the public side needs a trait
(or set of traits) that the private crate implements, plus a public default
implementation, at least a stub, for `--no-default-features` builds.

Measured against today's call graph, that boundary would have to cover about
85 modules and about 3,150 call sites (§2.3), including:

- **Data model and storage** (`models`, `db`, `object_store`, `storage`,
  `oplog`, `store_locate`): the requirement graph itself. Without these the
  public build cannot run `aida list`, so a stub would make the public binary
  useless and none of the integration tests could run publicly.
- **Git plumbing** (`git_ops`, 811 call sites; `rebase`, `worktree_pool*`,
  `worktree_holders`, `work_commit`): used throughout the CLI.
- **Coordination** (`mailbox*`, `liveness`, `idle`, `pickability`,
  `dispatch_routing`, `allocation_*`, `lifecycle`, `conflict`, `team`, `node`).
- **Infrastructure helpers** (`fs_atomic`, `write_atomic`, `toml_quote`,
  `file_lock`, `process_probe`, `yaml_helpers`), which are utilities and
  belong in the public crate whatever happens.
- **Scaffolding and templates**, which are public by nature (§2.5).

A boundary that wide is just the whole crate behind a trait. A boundary that
leaves something worth keeping private requires **first naming the
proprietary capability**. The spike does not name one, and nothing in
`aida-core/src` is obviously separable today. Candidates, if any exist, would
be a narrow algorithmic layer such as advisor and routing heuristics
(`dispatch_routing`, `pickability` scoring, `analytics`) or hosted-service
integrations (`integrations/`, `transport_private`). A carve-out of that size
looks like this:

1. A public `aida-core` that keeps the models, storage, git and coordination
   code, plus a new `pub trait` per capability (for example
   `trait RoutingPolicy`) with a simple public default.
2. A private crate `aida-pro` that implements those traits, selected by an
   `aida-cli-lib` feature `pro` (default off), registered at startup.
3. Public CI builds and tests with `pro` off. Private CI builds the full binary
   and runs the `pro` tests. `release.yml` moves to the private repo (or is
   dispatched there), because shipped binaries would include `pro`.

The cost is proportional to the size of the carve-out, not the size of
`aida-core`. A single focused capability is roughly days of refactoring plus
the two-repo CI plumbing.

## 5. Options compared

| Option | Protects | Public can build from source | Coupled-PR tax | Fork PRs | Setup cost |
|---|---|---|---|---|---|
| A. Whole `aida-core` private, checked out in public CI with a token | future core code only | no | 25% of PRs become two-repo | broken (no secrets) | medium; source still leaks into public logs |
| B. Open core: broad trait boundary over all of `aida-core` | future core code | yes, but the binary is a stub | 25% of PRs | fine | very high (≈3,150 call sites) |
| C. Open core: narrow named `aida-pro` carve-out | that capability only | yes, fully functional | only PRs touching the carve-out | fine | low to medium |
| D. Private monorepo + exported public mirror | everything chosen | yes (exported tree) | none internally | imported back manually | medium; ongoing exporter |
| E. Don't split; stay public MIT | nothing | yes | none | fine | zero |

## 6. Recommendation

**Don't split yet (option E).** The supporting facts:

1. The existing `aida-core` is already public and MIT-licensed, with 966
   commits of history. Splitting does not re-privatise any of it. History
   scrubbing would break every SHA reference AIDA depends on while protecting
   nothing that already exists.
2. `aida-core` has no narrow interface to split along: about 85 of its roughly
   99 public modules are called from other crates, and 88% of core-source PRs
   also change a consumer. Options A and B add a two-repo coordination cost to
   a quarter of all PRs, and the merge machinery (`aida ship`, merge queue,
   trailer-driven completion, merge holds) assumes a single repository.
3. There are no outside contributors today (0 fork PRs in 200), so the
   fork-secret constraint only becomes relevant once there is something worth
   protecting.
4. No specific proprietary capability has been named. Without one, there is
   no boundary to design.

**If a proprietary capability appears**, use **option C**: a narrow private
`aida-pro` crate behind public traits, with a public default implementation,
public CI that never compiles it, and private CI plus releases from the
private repository. Use **option D** (private monorepo + public mirror)
instead only if the proprietary surface grows large enough that C's
coupled-PR tax starts to matter. Do not choose A: it leaks source into public
logs, breaks fork CI, and does not let users build from source.

A cheap step that keeps option C open without committing to it: move
`aida-core/templates/` into its own crate or directory (it is public by
nature), so any future carve-out does not have to untangle 245 template
files and 11 embed sites first. That is worth filing as a separate story
only if C becomes likely.

## 7. Decision the operator must make

> **What, specifically, is proprietary?**
>
> - **Nothing yet / the goal was general protection** → close this spike as
>   *don't split*. AIDA stays a single public MIT repository. Revisit only
>   when a concrete capability is planned.
> - **A named capability (for example advisor routing heuristics or a hosted
>   integration)** → approve option C. Name the capability; the follow-up is
>   a story to define its trait boundary and a story for the two-repo CI
>   (private build and release, public default-feature CI, dispatch status
>   gate).
> - **Most of the core going forward** → approve option D (private monorepo
>   with exported public mirror), and accept that the public repository becomes
>   a downstream export with synthetic history.
>
> Related decision: whether a future license change (for example BSL or
> another source-available license for new versions) would achieve the
> commercial goal more cheaply than any repository split. That is a legal and
> business decision outside this spike's scope, but it should be weighed
> before paying for C or D.
