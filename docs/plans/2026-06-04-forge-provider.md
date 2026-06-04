# Forge-provider abstraction — SPIKE-49 design + proof (slice 0 of EPIC-35)

- **Date:** 2026-06-04
- **Specs:** SPIKE-49 (this), EPIC-35 (parent), supersedes SPIKE-39
- **Status:** SKETCH FOR MASTER REVIEW — design + proof only, no refactor. Slice 1 (the refactor) is deferred until master signs off on this trait surface.
- **Author:** product agent

> This is the de-risk SPIKE for making GitLab a first-class forge. It (1) inventories the GitHub coupling empirically, (2) proposes the forge-provider trait, (3) picks the GitLab backend + proves what's provable without credentials. It does **not** touch any `gh` call site.

---

## Deliverable 1 — GitHub-coupling inventory (claims VERIFIED, not trusted)

### The epic's two claims, checked empirically

| Epic claim | Verdict | Evidence |
|---|---|---|
| "~181 `gh` call sites" | **Overcounted.** ~137 real invocation lines; the 181 was a substring count (`main.rs` alone has 196 `gh ` substrings, mostly hint/help/test text). | `rg -n '\bgh ' aida-cli/src \| rg -v '//\|test\|...' \| wc -l` → 137 |
| "the STORE is already forge-agnostic" | **CONFIRMED.** | `remote_branch_exists(repo, remote, branch)` takes the remote as a param; `git_ops.rs` / `object_store.rs` / `db/git_backend.rs` contain **zero** gh/GitHub code paths (one doc-comment mention only). The orphan store rides whatever `origin` is. |

### gh invocations grouped by operation (the trait's method list)

| gh operation | sites | → trait method |
|---|---|---|
| `gh pr merge` | 29 | `merge_change` |
| `gh pr view` | 19 | `change_status` + `change_for_branch` (linkage) |
| `gh run` | 12 | `ci_status` |
| `gh pr create` | 10 | `open_change` |
| `gh pr checkout` | 6 | `checkout_change` (mostly pure git) |
| `gh api` | 6 | escape hatch — mostly hints (`gh api /rate_limit`) + one poll |
| `gh pr list` | 5 | `list_changes` |
| `gh pr comment` | 1 | `comment` |
| `gh workflow` | 1 | `dispatch_ci` (edge; may stay GitHub-only initially) |

Densest files: `main.rs` (196 substrings, mostly text), `pr_ship.rs` (21), `auto_complete.rs` (17), `cli.rs` (13), `pr_rebase.rs` (11), `network_retry.rs` (10), `workflow_hints.rs` (9), `state_snapshot.rs` (9).

### Critical refinement: the coupling is THREE categories, not one

1. **Real subprocess invocations** (~the 9 operations above) → route through the `Forge` trait. This is the slice-1 refactor surface.
2. **Hint / help / error strings** that tell the *user* to run gh (e.g. "run `gh run view <id>`", "`gh api /rate_limit` then re-run"). Many of the `gh run`/`gh api` hits are these. → need **forge-aware hint text** (a `forge.cli_name()` + templated hints), NOT a trait method. Lighter, separable concern.
3. **Incidental** (doc comments, test assertions) → no change.

**Implication for sizing:** the refactor is smaller than "137 sites." It's ~9 operations behind a trait + a hint-text pass. `gh pr merge` (29) and `gh pr view` (19) dominate and should be refactored first.

---

## Deliverable 2 — the `Forge` trait surface (SKETCH)

### Data types

```rust
pub enum ForgeKind { GitHub, GitLab, None }

/// A PR (GitHub) or MR (GitLab). Forge-native id: PR number / MR iid.
pub struct ChangeRef { pub id: u64, pub url: String, pub branch: String, pub base: String }

pub struct OpenChange { pub branch: String, pub base: String, pub title: String, pub body: String, pub draft: bool }

pub enum ChangeState { Open, Merged, Closed }
pub enum ReviewDecision { Approved, ChangesRequested, ReviewRequired, None }
pub struct ChangeStatus {
    pub state: ChangeState,
    pub mergeable: bool,
    pub review: ReviewDecision,
    pub head_sha: String,
}

pub enum CiState { None, Pending, Running, Success, Failed }
pub struct CiStatus { pub state: CiState, pub url: Option<String>, pub failing_checks: Vec<String> }

pub enum MergeMethod { Squash, Merge, Rebase }
pub struct MergeResult { pub merged: bool, pub sha: Option<String>, pub method: MergeMethod }

pub enum CiTarget { Branch(String), Commit(String), Change(ChangeRef) }
```

### The trait

```rust
pub trait Forge {
    fn kind(&self) -> ForgeKind;
    fn cli_name(&self) -> &'static str;                 // "gh" | "glab" | "" — for hint text

    fn open_change(&self, req: OpenChange) -> Result<ChangeRef>;        // gh pr create
    fn change_for_branch(&self, branch: &str) -> Result<Option<ChangeRef>>; // gh pr view (linkage)
    fn change_status(&self, c: &ChangeRef) -> Result<ChangeStatus>;     // gh pr view (status)
    fn ci_status(&self, target: CiTarget) -> Result<CiStatus>;         // gh run
    fn merge_change(&self, c: &ChangeRef, m: MergeMethod) -> Result<MergeResult>; // gh pr merge
    fn comment(&self, c: &ChangeRef, body: &str) -> Result<()>;        // gh pr comment
    fn checkout_change(&self, c: &ChangeRef) -> Result<()>;            // gh pr checkout
    fn list_changes(&self, filter: ChangeFilter) -> Result<Vec<ChangeRef>>; // gh pr list
}
```

### Forge-specific vs incidental (per method)

| Method | Forge-specific? | Notes |
|---|---|---|
| `open_change` | Yes | GitHub: `gh pr create`. GitLab: push options (token-free!) or `glab mr create`. |
| `change_status` | Yes | review decision + mergeable are forge concepts. |
| `merge_change` | Yes | merge method + auto-merge semantics differ per forge. |
| `ci_status` | Yes | GitHub Actions runs vs GitLab pipelines. |
| `comment` / `list_changes` | Yes | thin forge calls. |
| `checkout_change` | **Mostly incidental** | it's `git fetch` + checkout of the ref; only the ref-resolution is forge-flavored. Candidate to implement in pure git for all forges. |

### The `none` / pure-git provider — degraded semantics (this is what makes a GitLab project usable *today*, pre-drain)

| Method | pure-git behavior |
|---|---|
| `kind` / `cli_name` | `None` / `""` |
| `open_change` | No-op returning a synthetic `ChangeRef{ id:0, url:"", branch, base }` — "the branch is the change." Drain skips PR-gated phases. |
| `change_status` | Computed from **git ancestry**: `state = Merged` iff the branch tip is an ancestor of the default branch (`git merge-base --is-ancestor`); else `Open`. `review = None`, `mergeable = true`. **No forge needed.** |
| `ci_status` | `CiState::None` — drain treats like `lifecycle:no-ci-wait`. |
| `merge_change` | Fast-forward / `--no-ff` merge to the default branch via git directly (or no-op if the project works direct-to-`main`). |
| `comment` | No-op (log only). |
| `checkout_change` | Pure `git checkout` of the branch. |
| `list_changes` | Local branches, or empty. |

> The elegance: in pure-git mode, "is this merged?" is an ancestry query, and the `(SPEC-ID)`-trailer auto-complete already keys off commits landing on the default branch. So the existing lifecycle works with `none` and **no forge integration at all** — which is exactly the "leave it to pure git" path for projects that don't want PR/MR.

---

## Deliverable 3 — GitLab backend choice + round-trip proof

### Decision: `glab` CLI (primary), REST API (fallback for gaps)

| Axis | `glab` CLI | REST API (curl + PAT) |
|---|---|---|
| Consistency with codebase | **Mirrors the existing `gh`-shelling pattern** — each provider is a thin CLI wrapper | Asymmetric (HTTP for GitLab, CLI for GitHub) |
| New code surface | Low — parse `--output json` | Higher — HTTP client, pagination, error mapping |
| Self-hosted | `glab auth login --hostname gitlab.joemooney.com` | base-URL config |
| Pipelines/CI | `glab ci status` / `glab ci view` | `GET /projects/:id/pipelines` |
| Auth (headless drain) | `GITLAB_TOKEN` env or `glab auth` | `GITLAB_TOKEN` / PAT |
| Dependency | extra binary (like `gh` already is) | none |

**Recommendation:** GitLab provider shells out to **`glab`**, symmetric with the GitHub provider shelling to `gh`. It minimizes new code, matches the established pattern, and `glab`'s JSON output parses cleanly. Keep a **REST-API fallback** behind the same trait for any operation `glab` covers poorly. Document `glab` as a required dependency for GitLab projects (parallel to `gh` for GitHub). Master may prefer pure-REST to drop the binary dependency — flagged as the main open decision.

### Round-trip proof (against gitlab.joemooney.com, live)

**Environment:** GitLab up (HTTP 302 via Cloudflare), SSH authed as `@joe` on `:2222`, **`glab` NOT installed, no PAT available to this agent.**

| Leg | Status | Command / evidence |
|---|---|---|
| **Open MR** | ✅ **PROVEN LIVE — token-free** | `git push -o merge_request.create -o merge_request.target=main -o merge_request.title="..."` → created `https://gitlab.joemooney.com/joe/aida-gl-test/-/merge_requests/1`. **No API token needed.** |
| **Read status + pipeline** | ⏳ pending PAT | `glab mr view 1 --output json` (state, mergeable, head_sha) + `glab ci status` — or `GET /api/v4/projects/:id/merge_requests/1` + `.../pipelines`. Needs `GITLAB_TOKEN`. |
| **Merge MR** | ⏳ pending PAT | `glab mr merge 1 --squash --yes` — or `PUT /api/v4/projects/:id/merge_requests/1/merge?squash=true`. Needs `GITLAB_TOKEN`. |

**Design takeaway from the proof:** GitLab MR *creation* is achievable over pure SSH push options with **zero token** — so the GitLab `open_change` impl can be token-free, and only `change_status` + `merge_change` strictly require auth. That lowers the auth barrier for the most common drain entry point.

**To finish the round-trip:** a short-lived GitLab PAT (`api` scope) → run the status + merge commands above. Fixture left in place: `joe/aida-gl-test` (+ open MR `!1`).

---

## Refined EPIC-35 slice breakdown (sizes + order)

Refined from the inventory (smaller than feared; `none`-provider falls out of slice 1; merge+view dominate).

| Slice | Scope | Size | Notes |
|---|---|---|---|
| **1** | `Forge` trait + `[forge]` config + init auto-detect from origin host + **GitHub provider (gh, behavior-preserving) + `none`/pure-git provider**. Route the ~9 operations through the trait. Pure refactor — GitHub byte-identical. | **L** | Ships pure-git mode for free. Do `gh pr merge` (29) + `gh pr view` (19) first. Dogfood-safe (no behavior change). Needs master sign-off (architecture-class). |
| **2** | Forge-aware **hint/error text** pass (`cli_name()` + templated hints replacing hard-coded `gh …` strings). | **S** | Separable from slice 1; low risk. |
| **3** | **GitLab provider** — `open_change` (push options, token-free), `change_status`, `merge_change`, `comment`, `list_changes` via `glab` (+ REST fallback). | **L** | The core GitLab work. |
| **4** | **GitLab CI** — `ci_status` via `glab ci` / pipelines, wired into the drain's CI-wait (replacing the Actions wait). | **M** | |
| **5** | End-to-end: full `--auto-complete` drain on a GitLab project + MR linkage in `aida show` (TASK-241 PR→MR) + docs + the init forge-detection UX. | **M** | Validation slice; uses `joe/aida-gl-test`. |

Ordering rationale: slice 1 is the keystone (everything routes through the trait; GitHub unchanged; pure-git usable). Slice 2 is a cheap parallel win. Slices 3→4→5 build GitLab depth in dependency order (provider → CI → end-to-end).

---

## SPIKE-39 — recommend dismissal (for master)

SPIKE-39 ("Abstract forge integration (gh vs glab) so AIDA composes with GitLab CI/CD", Approved, Low) is the **predecessor** of this work and is **fully subsumed** by EPIC-35 + SPIKE-49: same goal (forge trait, gh vs glab, GitLab CI), now superseded by a concrete epic with a verified inventory, trait sketch, backend decision, and slice plan. **Recommend master reject SPIKE-39 as superseded-by EPIC-35** (not dismissing it here — master's call).

---

## Open decisions for master

1. **glab-CLI vs pure-REST** for the GitLab provider (this doc recommends glab+REST-fallback; REST-only drops the binary dep).
2. Trait naming: `ChangeRef`/`open_change` (forge-neutral) vs keeping `Pr*` names. This doc uses neutral names.
3. Whether `checkout_change` is implemented once in pure git (recommended) rather than per-forge.
4. `gh workflow` dispatch (1 site) — defer to GitHub-only initially?
