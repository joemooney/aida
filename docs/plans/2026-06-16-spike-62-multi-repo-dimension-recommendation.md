# SPIKE-62 — Multi-repo → one `.aida-store`: repo/component dimension (design recommendation)

- **Date:** 2026-06-16
- **Specs:** SPIKE-62 (this doc). References SPIKE-10 (subsystem-scoped advisors). Tier-1 trap = separate BUG-568 (out of scope here).
- **Status:** Recommendation. Tier 2 is buildable near-term (needs operator sign-off on field shape). Tier 3 is DO-NOT-BUILD-YET (gated — see §7).
- **Complexity:** Architecture-class. Touches the `Requirement` schema, git-linkage qualification, scope/queue/advisor model. Decisions here are hard to reverse once stores carry the new field.
- **Builds on:** `docs/plans/2026-06-15-multi-repo-one-store-review.md` (four code-reading passes). This doc resolves that review's open forks into concrete recommendations; it does not redo the code reading.

## 0. What this doc decides

The 2026-06-15 review established the ground truth: the **store layer already supports N-repos→1-store** (`--sibling` + `WorkspaceManifest`; conflict-free distributed IDs via HLC + node-namespaced dispenser + rebuildable per-clone cache). The **gap** is (1) the scanning layer is single-`project_root` (BUG-568 owns the Tier-1 loud-warning — NOT covered here), and (2) there is **no repo/component dimension in the model**. SPIKE-62 owns **Tier 2 (schema)** and **Tier 3 (component dimension)**.

This doc answers six questions:
- (a) field shape for the repo/component dimension,
- (b) how `completion_sha` / `branch` / `shipped_pr` get repo-qualified,
- (c) whether repo-identity and subcomponent are one dimension or two,
- (d) how the dimension feeds scope/queue/advisor (incl. SPIKE-10),
- (e) the migration / backward-compat story,
- (f) the forks: repo-identity vs node-id, one dimension vs two.

## 1. Recommendation in one line

Add **one optional, nested `origin` struct** on `Requirement` carrying an explicit `repo` slug plus an optional `component`; **repo-qualify linkage** by attaching the same `repo` slug to each commit/branch/PR record (not by reshaping the bare SHA); treat **repo and component as two levels of one hierarchical dimension** (a repo *contains* components); keep **repo-identity an explicit field, NOT node-id** (node-id is machine-scoped, not repo-scoped). Tier 2 (the field + linkage qualification) is buildable now; Tier 3 (the dimension flowing into queue/advisor scope) is recommended-but-gated.

## 2. (a) Field shape — nested `origin` struct (RECOMMENDED)

### Options considered

1. **Single `repo: Option<String>` on `Requirement`.** Cheapest. But it bakes the assumption that repo == the only sub-dimension, which forecloses the component level (and the review explicitly flags component as the harder, real question). Adding component later means a second field and a second migration — worse than getting the shape right once while the field is still absent everywhere.
2. **Reuse `feature` or `tags`.** Rejected. `feature` is an ID-prefix grouping (drives `spec_id` derivation via `prefix_override`/`feature`) — overloading it with repo identity collides two semantics and would leak into ID minting. `tags` (`HashSet<String>`) is freeform with no enforcement, no canonical key, no place to hang structured sub-fields (component, forge, default-branch). Cross-repo linkage disambiguation needs a *canonical, validated* key, not a freeform string. Tags remain fine for ad-hoc cross-cutting labels; they are the wrong home for an identity dimension.
3. **Nested `origin` struct (RECOMMENDED).**

   ```rust
   /// trace:SPIKE-62 — where this requirement's code lives.
   /// None/absent = single-repo store (the default; never breaks existing stores).
   #[serde(default, skip_serializing_if = "Option::is_none")]
   pub origin: Option<Origin>,

   pub struct Origin {
       /// Canonical repo slug, workspace-local (e.g. "api", "web").
       /// Matches a repo entry in the WorkspaceManifest. Validated lowercase/slug.
       pub repo: String,
       /// Optional finer-grained component within the repo (e.g. "orchestrator").
       /// Free within a repo's declared component set; None = whole repo.
       #[serde(default, skip_serializing_if = "Option::is_none")]
       pub component: Option<String>,
   }
   ```

### Why nested wins

- It encodes the **two-level hierarchy** (repo → component, §4) in one field, so Tier 3 needs no second schema migration — only behavior on top of an already-present shape.
- It gives linkage a **canonical key** (`origin.repo`) to qualify SHA/branch/PR against (§3).
- It is **backward-compatible by construction**: `Option<Origin>` + `serde(default, skip_serializing_if = "Option::is_none")` means existing YAML with no `origin:` key deserializes to `None`, serializes back without the key, and every existing single-repo store round-trips byte-for-byte. `None` is defined to mean "the one repo this store has always assumed" — i.e. current behavior.

### Validation / canonicalization

`origin.repo` must resolve against the `WorkspaceManifest` repo list (`aida-core/src/workspace.rs::WorkspaceManifest`) when a manifest exists; in single-repo stores there is no manifest and `origin` stays `None`. Recommend a lowercase-slug constraint mirroring `prefix_override`'s uppercase-letters validation pattern, so the slug is stable as a map key and a CLI filter value.

## 3. (b) Linkage qualification — qualify the *record*, not the SHA

Today (verified in `aida-cli/src/main.rs`):
- `GitLinkage` carries `commits: Vec<(full_sha, short_sha, subject)>`, `branch: Option<String>`, `shipped_pr: Option<u64>` — **no repo/forge anywhere**.
- `ImplementationInfo.completion_sha: Option<String>` (`aida-core/src/models.rs`) is a **bare SHA**, stamped alongside `completed_at` by the `aida pull` auto-bump.

A bare SHA is globally near-unique, so the ambiguity is not collision — it is **provenance**: given a SHA you cannot tell *which repo* to `git -C` into to resolve it, and `branch`/`shipped_pr` (a branch name, a PR number) are only unique *within a forge/repo*. PR #141 means nothing without knowing the repo and forge.

### Recommendation

Do **not** reshape `completion_sha` into a compound string (e.g. `"repo@sha"`) — that's a parsing-hazard and breaks any reader that treats it as a SHA. Instead **carry the repo slug as a sibling**:

- **`ImplementationInfo`**: add `completion_repo: Option<String>` next to `completion_sha`/`completed_at`. The auto-bump path stamps the slug of the repo whose default-branch commit triggered the bump. `None` = single-repo / legacy (resolve against the sole repo, current behavior).
- **`GitLinkage`**: this is an in-memory scan result, not persisted schema, so widen it freely. Make commit/branch/PR records repo-attributed — minimally tag the struct with the `repo` it was scanned from (when the scanner iterates the workspace manifest, §5), and qualify `shipped_pr` with its forge/repo context so `print_git_linkage` can render "api#141" vs "web#141" unambiguously.
- **Forge resolution**: `origin_url` / `change_lookup_for_branch` (forge.rs, main.rs) must key off the *scanned repo's* origin, not the local cwd's origin — already implied by the Tier-1 multi-repo scan iteration; SPIKE-62's contribution is that the `origin.repo` slug is the join key tying a spec to the right repo to scan.

The invariant: **`origin.repo` on the spec is the single canonical slug; every linkage artifact (completion record, commit set, branch, PR) is qualified by that same slug.** One vocabulary, used on both the spec and its linkage.

## 4. (c) Repo vs component — TWO LEVELS OF ONE DIMENSION (a repo contains components)

The fork: is repo-identity the same dimension as subcomponent, or two?

**Resolution: one hierarchical dimension with two levels — `repo` is the coarse level, `component` the fine level; a repo contains components.** Arguments:

- **They nest cleanly, they don't cross.** A component belongs to exactly one repo. There is no "component spanning two repos" case in the operator's framing (a group of repos under one AIDA view). So this is a containment hierarchy, not two orthogonal axes — which is exactly what the nested `Origin { repo, component }` encodes.
- **Repo is physically grounded; component is logical.** Repo maps to a real `WorkspaceManifest` entry / git origin (it must, for linkage to resolve — §3). Component is an advisor/scope convenience with no physical backing. Modeling them as one nested field lets repo stay validated-against-manifest while component stays free-form-within-repo.
- **SPIKE-10 wants the fine level even inside a monolith.** SPIKE-10's gap #1 ("subsystem-advisor within a monolithic repo") is the *single-repo, multi-component* case: `subsystem: orchestrator` with no second repo. The two-level model serves that directly — `origin = { repo: <the one repo>, component: orchestrator }` (or component-only semantics where repo is implicit). A flat single-`repo` field could not express it; two unrelated fields would over-model it. The hierarchy is the right altitude.
- **Filtering composes naturally.** "All specs in repo `api`" = filter on `origin.repo`; "all specs in `api/orchestrator`" = filter on both. A hierarchy gives you both granularities from one field.

So: **not the same dimension, but not two independent dimensions either — one dimension, two levels.**

## 5. (d) How it feeds scope / queue / advisor

The dimension is **passive metadata in Tier 2** and becomes an **active scope key in Tier 3**.

- **Tier 2 (now): display + linkage join only.** `origin` is shown in `aida show`, and `origin.repo` is the join key the (BUG-568) multi-repo scanner uses to pick the right repo to `git -C` into for a given spec. No filtering behavior, no queue changes. This is the safe, buildable slice.
- **Tier 3 (gated): scope key into list/search/queue/advisor.**
  - **list/search**: an `--repo <slug>` / `--component <name>` filter on `aida list` / `aida search` (and the MCP `list_requirements` / `search_requirements`), filtering on `origin`.
  - **queue**: route/filter the work queue by `origin`. Note the review's standing caveat — the queue is **user-keyed** (`registry/queues/{user}.yaml`, BUG-89), and multi-repo *amplifies* the existing same-user contention (TASK-618/BUG-220 history). Tier 3 queue scoping must be designed *with* that contention fix, not bolted onto it. This coupling is a primary reason Tier 3 is gated.
  - **advisor (SPIKE-10 linkage)**: `origin.component` is the natural backing for SPIKE-10's `subsystem:` scope. A `--focus <component>` advisor session (SPIKE-10 gap #1) loads universal discipline + specs/memories whose `origin.component` matches. `origin.repo` is the backing for SPIKE-10's sibling-advisor initiation (gap #2): a per-repo advisor scoped to one repo's slice of the shared store. **The component level only earns its keep once SPIKE-10's subsystem-advisor work is real** — which is itself why Tier 3 is gated, not speculative.

The clean story: **Tier 2 plants the field and the linkage join; Tier 3 turns that same field into a scope key for queue and advisor once there's a real consumer.**

## 6. (e) Migration / backward-compat — a no-op for existing stores

- **No data migration required.** `origin: Option<Origin>` absent = `None` = single-repo, which is the current and forever-default behavior. Existing YAML round-trips unchanged (`skip_serializing_if = "Option::is_none"`). `completion_repo: Option<String>` absent = "the sole repo," matching today's bare-SHA resolution.
- **No backfill.** Existing specs stay `origin: None`. They are correct as-is — they *are* single-repo. Backfilling a slug onto historical specs is unnecessary and risky; leave them `None`.
- **Opt-in population.** `origin` gets set only when a store actually becomes multi-repo (a `WorkspaceManifest` exists and the operator/advisor assigns specs to repos). Until then the field is inert.
- **Forward-compat for old binaries:** a store written by a new binary with `origin:` present will be read by an old binary that ignores the unknown key (serde default behavior) — graceful degradation, no crash. Worth a confirming test, mirroring `detect_distributed_store_resolves_sibling_store_outside_repo`'s style.

## 7. GATE — Tier 3 is DO-NOT-BUILD-YET

**Tier 2 (the `origin` field + `completion_repo` + linkage qualification) is the buildable near-term slice.** It is backward-compatible, small, and unblocks BUG-568's scanner (gives it the join key) without committing to scope/queue/advisor mechanics.

**Tier 3 (the dimension feeding queue/advisor scope) MUST be gated** on:
1. **A real multi-repo project actually in use.** The operator named the multi-repo use case as *prospective*, not live. Building queue/advisor scoping with zero real consumer is speculative architecture — exactly what the review's anti-overengineering note warns against. The dispenser/cache foundation means we are **not** racing a costly retrofit; the expensive part is already paid, so there is no urgency tax for waiting.
2. **SPIKE-10 subsystem-advisor work being real.** The component level's payoff is advisor scoping; without SPIKE-10 it is a filter with no consumer.
3. **The user-keyed queue contention fix (BUG-89 lineage) being designed in tandem.** Tier 3 queue scoping inherits that hazard; do not layer scope routing on a contended substrate.

## 8. (f) Forks resolved

### Fork 1 — repo-identity == node-id, or explicit field?

**Explicit field. Repo-identity is NOT node-id.** Node-id (`load_node_id`, `dispenser.rs`, embedded in distributed `spec_id` minting) is **machine/clone-scoped**, not repo-scoped: it identifies *which writer* allocated an ID, for conflict-free distribution. Two clones of the *same* repo on two machines have *different* node-ids; one machine working in *two* repos uses *one* node-id across both. Either way node-id does not partition by repo. Reusing it as repo identity would be a category error and would couple repo identity to the ID-allocation machinery (a refactor hazard). Repo identity is a first-class, explicit `origin.repo` slug, validated against the `WorkspaceManifest`.

### Fork 2 — one dimension (repo == component) or two (repo contains components)?

**One hierarchical dimension, two levels: a repo contains components.** See §4. Encoded as `Origin { repo, component: Option<_> }`. Not flat (can't express subsystem-within-monolith for SPIKE-10), not two orthogonal axes (components never span repos).

## 9. Decisions that need the operator (master advisor) before any PR

This is architecture-class; flag for sign-off:
- **D1.** Approve the **nested `Origin` field shape** over a flat `repo` string (§2). Once stores carry `origin`, the shape is hard to change.
- **D2.** Approve **`completion_repo` as a sibling to `completion_sha`** rather than reshaping the SHA (§3).
- **D3.** Confirm **repo + component as one hierarchy, not two dimensions** (§4) — this is the load-bearing modeling call.
- **D4.** Confirm **Tier 3 stays gated** on the three conditions in §7 (real multi-repo project + SPIKE-10 + queue-contention fix). Tier 2 may proceed on D1–D2 approval.
- **D5.** Confirm the **`origin.repo` slug vocabulary is the single join key** shared by spec and all linkage artifacts (§3 invariant).

## 10. Sequencing

1. (Separate) BUG-568 — Tier-1 loud-warning when a referencing repo isn't visible. Independent of this doc.
2. **Tier 2-a** — add `Origin` + `Requirement.origin`; serde-default/skip; validation against `WorkspaceManifest`; round-trip + old-binary-forward-compat tests. (Gated on D1, D3.)
3. **Tier 2-b** — add `ImplementationInfo.completion_repo`; stamp it in the auto-bump path; widen in-memory `GitLinkage` to carry per-repo attribution; qualify `shipped_pr`/branch rendering in `print_git_linkage`. (Gated on D2, D5; benefits from BUG-568's manifest-iterating scanner.)
4. **Tier 3** — DO NOT BUILD until §7 gate clears. When it does: `--repo`/`--component` filters on list/search/queue + MCP equivalents, advisor `--focus` backing for SPIKE-10, co-designed with the queue-contention fix.

## Critical Files for Implementation
- `aida-core/src/models.rs` (`Requirement`, `ImplementationInfo` — new `Origin` + `completion_repo`)
- `aida-cli/src/main.rs` (`GitLinkage`, `collect_git_linkage_opts`, `print_git_linkage`, `auto_bump_done_to_completed` — linkage qualification + auto-bump stamping)
- `aida-core/src/workspace.rs` (`WorkspaceManifest` — `origin.repo` validation source)
- `aida-cli/src/cli.rs` (`--repo`/`--component` filter flags — Tier 3, gated)
- `docs/plans/2026-06-15-multi-repo-one-store-review.md` (the review this builds on)

<!-- trace:SPIKE-62 | ai:claude -->
