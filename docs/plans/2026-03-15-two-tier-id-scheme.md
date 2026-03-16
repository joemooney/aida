# Two-Tier ID Scheme: Design & Rationale

**Date**: 2026-03-15
**Status**: Design — not yet implemented
**Branch**: `distributed-architecture`
**Related Spec**: Section 5 of `2026-03-15-distributed-architecture-identity.md`

## The Problem

In distributed mode, IDs look like `FR-7-048` — "FR type, node 7, sequence 48". Unambiguous and collision-free, but awkward in daily use:

> "Hey, what's the status of FR forty-eight?"
> "Which node's FR-48? Node 7's or node 11's?"

Every successful tracking tool converges on short IDs: Jira's `ENG-423`, Linear's `ENG-423`, GitHub's `#423`. Teams resist longer IDs. This is a social constraint that kills adoption.

## The Solution: Two Tiers

### Tier 1 — Node ID (assigned at creation, always)

```
FR-7-048
```

Assigned by the local dispenser at object creation. Immutable from that moment. Valid forever, in all contexts, on all branches. No network required.

### Tier 2 — Agreed ID (assigned at merge-to-trunk)

```
FR-423
```

A short global sequential integer assigned when a feature branch merges to trunk. Stored as an additional `agreed_id` field on the object. Both IDs permanently resolve to the same UUID. Neither is deprecated.

## Merge Gate Procedure

A shared counter file in the registry:

```toml
# registry/agreed_counters.toml
FR = 422
FEAT = 89
TEST = 341
```

When a feature branch merges to trunk:

```
git merge feature/auth-refactor
    ↓
aida merge-gate
    ↓
For each new object in the branch lacking an agreed_id:
    Read counter for this type (e.g., FR = 422)
    Assign agreed_id = "FR-423"
    Increment counter to 423
    Write agreed_id into the object YAML
    ↓
CAS push to registry (retry on rejection — same as node registration)
    ↓
Merge proceeds
```

The merge gate is the only point where central coordination is required after initial node registration. Merging to trunk is inherently a connected operation — this is not a new constraint.

## Immutability Rule

**Identifiers embedded in committed source code are immutable from creation. No renumbering, ever.** (Spec G-01)

When you write a trace comment on a feature branch:

```rust
// trace:FR-7-048 | ai:claude
fn validate_token() { ... }
```

That `FR-7-048` stays in the code **forever**. There is never a find-and-replace to change it to `FR-423` after merge. That would:

- Rewrite git blame history
- Create noisy diffs across dozens of files
- Break if anyone has a local branch referencing the old ID
- Be a correctness violation, not a maintenance task

## What Happens at Merge

The merge gate assigns `agreed_id` as **metadata on the object** in the YAML file:

```yaml
# objects/FR/000/FR-7-048.yaml
id: FR-7-048           # never changes
agreed_id: FR-423      # added at merge
uuid: 018e7f3a-...
title: OAuth2 token validation
```

The source code is untouched. Commit messages are untouched. Git history is sacred.

## Resolution Logic

All three identifiers resolve to the same object:

```bash
aida show FR-7-048     # by node ID (always works)
aida show FR-423       # by agreed ID (works after merge)
aida show 018e7f3a...  # by UUID (always works)
```

Lookup order:
1. Try exact match on `spec_id` (the node-namespaced ID)
2. Try match on `agreed_id`
3. Try match on `uuid`

## What People Write in Code and Commits

**Before merge** (on feature branch):
```rust
// trace:FR-7-048 | ai:claude
```
```
git commit -m "[AI:claude] feat(auth): add validation (FR-7-048)"
```

**After merge** (new code on trunk):
```rust
// trace:FR-423 | ai:claude
```
```
git commit -m "[AI:claude] fix(auth): handle expired tokens (FR-423)"
```

Both are valid forever. New work on trunk **prefers** the short form because it's available and more readable. Old references from the feature branch remain as `FR-7-048` and are never updated.

The agreed ID is not a rename — it's an **alias added at merge**. The original ID is never replaced, only augmented. Tooling resolves both forms transparently.

## Display Behavior

When listing requirements, the agreed ID is shown as the primary identifier when available:

```
$ aida list
ID        Node ID      Type        Status     Title
FR-423    FR-7-048     Functional  Approved   OAuth2 token validation
FR-424    FR-7-049     Functional  Draft      Session management
─         FR-11-003    Functional  Draft      (not yet merged)
```

The third row has no agreed ID — it hasn't been merged to trunk yet.

## Objects Never Merged

Scratch work, abandoned branches, exploratory requirements — they never receive an agreed ID. This is semantically correct: the agreed ID signals "reviewed and ratified via the merge process." An object that never completes that process simply remains identified by its node-namespaced ID and UUID for its lifetime. No cleanup or forced ratification policy is required.

## Usage by Context

| Context | Preferred ID | Reason |
|---|---|---|
| Feature branch (pre-merge) | `FR-7-048` | Agreed ID not yet assigned |
| Code comments on trunk | `FR-423` | Short, pronounceable, Jira-like |
| Cross-reference in any doc | Either | Both resolve to same UUID |
| Verbal standup reference | `FR-423` | Globally unambiguous |
| git blame / provenance | `FR-7-048` | Shows which node created it |

## Stale Inline Descriptions

The spec (Section 2.4, Mode B) allows optional inline descriptions:

```rust
/// [FR-423] OAuth2 token validation
```

If the requirement title changes later, the inline description becomes stale. But the **ID is still correct** — the description is informational only. `aida audit --stale-descriptions` can find and suggest updates. The diff is cosmetic (ID unchanged), safe to apply in bulk.

---

## When Are Node-Namespaced IDs Actually Needed?

This is the critical question for practical deployment.

### If You Always Have a Central Server

**You don't need node-namespaced IDs at all.**

When the central database (PostgreSQL or even SQLite on a shared server) is always reachable, the server itself is the single source of sequence numbers. There's no collision risk because there's one authority. IDs can be simple `FR-001`, `FR-002` — exactly what AIDA does today in centralized mode.

The entire node-namespaced scheme (`FR-7-048`) exists to solve one specific problem: **what happens when two developers create requirements simultaneously without being able to coordinate?** If they can always coordinate (via a central server), the problem doesn't exist.

### When You Actually Need Node-Namespaced IDs

Node-namespaced IDs are required only when **all three** of these conditions are true:

1. **Multiple people** are creating requirements (not solo use)
2. **No shared server** is reachable at creation time (offline, air-gapped, or unreliable network)
3. **IDs must be assigned immediately** at creation (can't defer until connectivity returns)

Real-world scenarios where this applies:
- Air-gapped classified networks where teams operate on disconnected LANs
- Field teams with intermittent connectivity (ships, remote sites)
- Multi-site development with site-to-site sync but no always-on central service

### The Hybrid Approach: Configurable Per Deployment

This is what AIDA should support — **two modes, same codebase**:

| Mode | IDs | When to use |
|---|---|---|
| **Centralized** | `FR-001` | Team has reliable connectivity to a central server. Default. |
| **Distributed** | `FR-7-001` → `FR-423` at merge | Team operates offline or across disconnected networks. |

The `DeploymentMode` enum (already implemented) handles this:

```rust
enum DeploymentMode {
    Centralized,                    // FR-001 — simple, no node prefix
    Distributed { aida_repo_path }, // FR-7-001 — node-namespaced
}
```

### What This Means for Agreed IDs

In **centralized mode**, agreed IDs are unnecessary — `FR-001` is already the short, pronounceable form. There's nothing to "agree" on because the server assigned it authoritatively.

In **distributed mode**, agreed IDs are the upgrade path: `FR-7-048` on the branch → `FR-423` on trunk. This gives distributed teams the same short-ID experience that centralized teams get by default.

| Mode | Creation ID | Trunk ID | Needs merge gate? |
|---|---|---|---|
| Centralized | `FR-423` | `FR-423` | No — already short |
| Distributed | `FR-7-048` | `FR-423` (agreed) | Yes — assigned at merge |

### Recommendation for Most Teams

**Start centralized.** Use `aida init` (not `--distributed`). Get simple `FR-001` IDs. If you later need offline/disconnected operation:

1. Export to a git-backed store: `aida db export-git -o aida-store`
2. Switch to distributed mode
3. Node-namespaced IDs kick in for new objects
4. Existing `FR-001` IDs remain valid forever (they're already in the agreed ID format)

The distributed architecture is insurance for edge cases, not a requirement for adoption.

---

## Implementation Plan

### What needs to be built

1. **`agreed_id` field** on the `Requirement` struct — `Option<String>`, serde-skipped for centralized mode
2. **`AgreedCounters`** — already implemented in `node.rs`
3. **`aida merge-gate` command** — runs at merge time, assigns agreed IDs via CAS push
4. **Resolution logic** — `get_requirement_by_spec_id()` searches both `spec_id` and `agreed_id`
5. **Display preference** — show `agreed_id` when available, fall back to `spec_id`
6. **`aida show`/`aida list`** — display both IDs in relevant contexts

### What already exists

- `AgreedCounters` struct with `next()`, `peek()`, `format_agreed_id()` — in `node.rs`
- `DeploymentMode` enum — centralized vs distributed
- CAS push loop — proven in `register_node()`
- Sharded YAML storage — `agreed_id` just becomes another field in the YAML

### What does NOT need to change

- Source code trace comments — never rewritten
- Commit messages — never rewritten
- Git history — never rewritten
- Existing centralized IDs — already in the short format
