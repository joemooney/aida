# Aida — Distributed Architecture & Identity Specification

**Version:** 0.5
**Status:** DRAFT
**Date:** March 2025

> Requirements management as a distributed system. Git as the event log. Postgres as a disposable view. IDs that are immutable from birth.

## Related Requirements

- To be created as implementation begins

## Status

In Progress — Branch: `distributed-architecture`

## Evaluation Notes

This spec was evaluated against the current AIDA architecture on 2026-03-15. Key decisions:

- **Dual-mode operation**: Support both centralized (PostgreSQL, simple IDs) and distributed (git-as-event-log, node-namespaced IDs) modes, configurable per deployment.
- **Git scaling assessment needed**: Early spike required to test git performance with 10K-100K TOML object files. If problematic, alternative: git tracks operations/deltas, objects reconstructed into local SQLite/PostgreSQL.
- **ID scheme**: Node-namespaced IDs (`FR-7-048`) only needed in distributed mode. Centralized mode retains simple IDs (`FR-423`). Both modes share UUID v7 as canonical machine identity.
- **Migration timing**: Intentionally done now while codebase is young (~166K lines, 396 requirements, single user) to minimize migration cost.

---

## Table of Contents

1. [Purpose & Constraints](#1-purpose--constraints)
2. [Object Identity](#2-object-identity)
3. [Node ID Assignment via Git CAS](#3-node-id-assignment-via-git-cas)
4. [The Sequence Dispenser](#4-the-sequence-dispenser)
5. [Two-Tier ID Scheme](#5-two-tier-id-scheme)
6. [Multi-Repo Workspace Architecture](#6-multi-repo-workspace-architecture)
7. [Storage Architecture](#7-storage-architecture)
8. [Synchronization Architecture](#8-synchronization-architecture)
9. [Real-Time Updates & Web UI Architecture](#9-real-time-updates--web-ui-architecture)
10. [CRDT & Conflict Resolution Strategy](#10-crdt--conflict-resolution-strategy)
11. [Implementation Phasing](#11-implementation-phasing)
12. [Technology Decisions](#12-technology-decisions)
13. [Type Prefix Vocabulary](#13-type-prefix-vocabulary)
14. [Open Questions](#14-open-questions)
15. [Gotchas & Design Traps](#15-gotchas--design-traps)

---

## 1. Purpose & Constraints

Aida manages requirements, features, files, functions, and any relatable engineering artifact as first-class objects with stable, globally unique identifiers in a distributed, occasionally-connected environment including air-gapped and classified network segments.

### Non-Negotiable Constraints

- **Identifiers embedded in source code and documentation are immutable from creation.** No renumbering, ever. A commit that renames `FR-7-048` to `FR-423` across forty source files is a correctness violation, not a maintenance burden.
- **Node registration requires network access. No exceptions.** A clone that has not completed registration cannot create any objects. There is no provisional ID state.
- **The system must function fully offline after registration** with no degradation of write capability.
- **Synchronization must be conflict-minimizing by construction**, not by coordination.
- **Git is the source of truth.** All other stores are derived, disposable projections.
- **Identifiers must be human-typeable and human-readable** at the terminal and in code review.
- **Sequence number dispensing is a local problem.** Once a Node ID is assigned, no network coordination is required to generate new object IDs.

---

## 2. Object Identity

### 2.1 The Identity Hierarchy

Four levels of identity exist. They serve different purposes and must not be conflated.

| Level | Scope | Assigned By | Permanence |
|---|---|---|---|
| Workspace | Project / organization | Central server at project creation | Permanent |
| User ID | Human actor | Central server at account creation | Permanent — never reused |
| Node ID | Git clone instance | Git CAS push loop at `aida init` | Permanent once any ID committed |
| Agreed ID | Ratified trunk object | Merge gate CAS at merge-to-trunk | Permanent alias added at merge |

The **Node ID** is the atomic unit of ID generation. The **User ID** is an attribution concern, not an addressing concern. A user with three clones will have three node IDs, all attributed to the same user.

### 2.2 Canonical ID Format

```
{TYPE}-{NODEID}-{SEQ}
```

| Field | Description | Example values |
|---|---|---|
| `TYPE` | 2–6 character uppercase type prefix | `FR`, `FEAT`, `TEST`, `ARCH` |
| `NODEID` | Sequential integer node identifier (see Section 3) | `1`, `7`, `42` |
| `SEQ` | Zero-padded monotonic sequence, local to this node and type | `001`, `042`, `1001` |

**Both hyphens are mandatory structural delimiters, not cosmetic separators.**

Without them, `FR-111001` is ambiguous: node 1 seq 11001, node 11 seq 1001, or node 111 seq 001? With both hyphens as delimiters, `FR-11-001` (node 11, seq 1) and `FR-1-1001` (node 1, seq 1001) are unambiguous regardless of field length. There is no hard limit on the digit count of either field.

**Examples:**

```
FR-7-001        Functional Requirement #1 from node 7 (Joe's laptop)
FR-11-001       Functional Requirement #1 from node 11 (Alice's workstation)
FR-1-1001       Functional Requirement #1001 from node 1
FEAT-3-042      Feature #42 from node 3
TEST-14-003     Test case #3 from node 14
FILE-7-017      File reference #17 from node 7
```

### 2.3 UUID as Canonical Machine Identity

Every object carries a UUID v7 (time-ordered, RFC 9562) as its machine-canonical identity. The human-readable display ID is a user-facing convenience. The UUID is used for:

- Foreign key references in the Postgres read model
- Deduplication during sync
- Cross-workspace and cross-repo references
- CRDT vector clock keying

UUID v7 is preferred over v4 because it is time-ordered, which benefits storage locality and index performance.

### 2.4 Display Names in Code

The ID embedded in source code is always immutable. Whether a human-readable description accompanies it is a **project-level configuration option** with a documented trade-off.

**Mode A — ID only (default, strictest)**

```rust
/// [FR-7-048]
pub fn authenticate(token: &str) -> Result<Session> { ... }
```

The description is resolved at display time by Aida tooling (IDE plugin, CLI, web UI). Always current. Requires a tooling lookup.

**Mode B — ID with inline description (configurable)**

```rust
/// [FR-7-048] OAuth2 token validation
pub fn authenticate(token: &str) -> Result<Session> { ... }
```

The description is embedded at creation time for convenience — saves a lookup when reading code without tooling. Accepted trade-off: the inline description may become stale if the requirement title changes. The ID remains authoritative; the description is informational only.

Projects using Mode B should run `aida audit --stale-descriptions` periodically to identify and optionally update diverged inline descriptions. Because these are cosmetic changes (the ID is unchanged), the diff is clean and reviewable without concern about correctness.

After trunk merge, both the node-namespaced ID and the agreed ID (see Section 5) are valid in code. `FR-423` is preferred on trunk for brevity; `FR-7-048` remains valid forever as an alias.

---

## 3. Node ID Assignment via Git CAS

Node IDs are small sequential integers assigned once per clone at initialization. The assignment mechanism uses git push rejection as a distributed compare-and-swap (CAS), requiring no central coordination service beyond the shared git repository itself.

### 3.1 The Counter File

The aida repo maintains a shared counter file:

```toml
# aida/registry/node_counter.toml
next_node_id = 8
```

### 3.2 Assignment Procedure

When a user runs `aida init` on a new clone:

```
1. Ensure network connectivity — fail hard if unreachable
2. Pull latest aida repo
3. Read next_node_id from node_counter.toml  (e.g., 8)
4. Write node_id = 8 to local .aida/node.toml  (gitignored)
5. Increment next_node_id to 9 in node_counter.toml
6. Append registration entry to aida/registry/nodes.toml
7. git commit both changes
8. git push

IF push rejected (non-fast-forward — another clone registered first):
   git pull --rebase
   goto step 3

ELSE push succeeded:
   Node ID is confirmed. Clone is fully operational.
```

**The push success is proof of uniqueness.** The push rejection is the optimistic lock. The retry loop is bounded and fast — node registration is a rare, one-time event per clone. No separate validation service is required.

**A clone that cannot complete this procedure cannot create objects.** There is no provisional state. `aida init` without network access fails with a clear error message.

### 3.3 Node Registry

The committed registry maps node IDs to user attribution. It is append-only and merges without conflicts since nodes are only ever added:

```toml
# aida/registry/nodes.toml

[[node]]
id         = 7
user_id    = 102
hostname   = "joe-laptop"
registered = "2025-03-10"

[[node]]
id         = 8
user_id    = 102
hostname   = "joe-workstation"
registered = "2025-03-13"

[[node]]
id         = 9
user_id    = 7
hostname   = "alice-dev"
registered = "2025-03-13"
```

User 102 (Joe) has two clones: node 7 on his laptop, node 8 on his workstation. Both are attributed to the same user but generate non-conflicting IDs across the entire workspace.

### 3.4 The Node ID Invariant

Once any object ID prefixed with a given node ID has been committed to the aida repo, that node ID is permanently frozen. The `.aida/node.toml` file is gitignored — it does not travel with the repo when someone else clones it. Every new clone runs `aida init` independently and receives its own unique node ID.

---

## 4. The Sequence Dispenser

The sequence dispenser is a deliberately separated concern. Once a Node ID is assigned, the distributed uniqueness problem is **fully solved** — node 7's IDs cannot conflict with node 11's IDs by construction, regardless of how either node dispenses sequence numbers internally. The dispenser is therefore a **purely local, single-node problem**.

### 4.1 Interface

The dispenser's entire public interface is:

```rust
fn next(object_type: ObjectType) -> Result<u32>
fn peek(object_type: ObjectType) -> Result<u32>   // without incrementing
fn state() -> Result<DispatcherState>
```

No network. No coordination. No CAS loop. A monotonic counter that never goes backward. All Aida commands that create objects call this interface — never the counter storage directly.

### 4.2 Scope

- Keyed on `(node_id, type)` — not on repo
- **Global across all repos in the workspace for a given node**
- Node 7's FR counter increments whether the requirement originates in `pacgate` or `pacinet`
- Persisted locally in the gitignored node state

```toml
# .aida/node.toml  (gitignored — never committed)
node_id = 7
user_id = 102

[sequences]
FR   = 48      # next will be FR-7-049
FEAT = 12
TEST = 3
FILE = 203
ARCH = 1
```

### 4.3 Implementation Options

The dispenser interface is stable. The backing implementation can be migrated without touching any caller.

**Option A — File counter with lockfile (Phase 1)**

Read `.aida/node.toml`, acquire a lockfile, increment the relevant counter, write back, release the lock, return the value. Correct under all single-machine concurrency scenarios. Approximately one hundred lines of Rust.

```rust
fn next_seq(node_toml: &Path, object_type: &str) -> Result<u32> {
    let _lock = FileLock::acquire(node_toml.with_extension("lock"))?;
    let mut state = NodeState::load(node_toml)?;
    let seq = state.sequences.entry(object_type.to_string()).or_insert(0);
    *seq += 1;
    let result = *seq;
    state.save(node_toml)?;
    Ok(result)
}
```

**Option B — SQLite counter (Phase 2)**

One table, one row per `(node_id, type)`, updated atomically. SQLite's write serialization handles all concurrency. Natural fit since the local read model (Section 7) also lives in SQLite — same file, separate tables.

```sql
INSERT INTO sequences (node_id, type, next_val) VALUES (7, 'FR', 1)
ON CONFLICT (node_id, type) DO UPDATE SET next_val = next_val + 1
RETURNING next_val;
```

**Option C — Local daemon over Unix socket (Phase 3)**

A long-running `aida-daemon` process owns the counter state in memory and serves requests over a Unix domain socket. Handles concurrent callers — multiple CLI commands, IDE plugin, language server, background sync agent — all calling `next()` simultaneously without contention. See Section 4.4.

### 4.4 The aida-daemon: Avahi-Inspired Architecture

The daemon architecture follows the pattern established by **Avahi** — the Linux Zeroconf/mDNS implementation. Avahi's key design decisions translate directly:

- Single system daemon owns the shared local resource for its scope
- D-Bus / Unix socket as IPC — well-known path, any process can connect
- Thin client library (`libavahi-client`) that all callers use; internals are hidden
- Socket activation — daemon starts on first client connection, not at boot
- Graceful degradation when the network subsystem is unavailable

The Aida daemon adapts this pattern with two internal subsystems:

```
┌─────────────────────────────────────────────────────┐
│                   aida-daemon                        │
│                                                      │
│  ┌──────────────────┐   ┌────────────────────────┐  │
│  │   Dispenser      │   │  Presence / Discovery  │  │
│  │                  │   │                        │  │
│  │  next(type)      │   │  announce(working_on)  │  │
│  │  peek(type)      │   │  discover() -> peers   │  │
│  │  state()         │   │  on_conflict(callback) │  │
│  └────────┬─────────┘   └───────────┬────────────┘  │
│           │                         │                │
│           ▼                         ▼                │
│     SQLite / file             libavahi-client        │
│     counter store             (mDNS via system       │
│                                Avahi or equivalent)  │
└─────────────────────────────────────────────────────┘
           ↑
    Unix socket (/run/user/{uid}/aida.sock)
           ↑
┌──────────┴──────────────────────────────────────────┐
│         libaida-client  (thin Rust crate)            │
│  used by: CLI, IDE plugin, LSP server, sync agent    │
└─────────────────────────────────────────────────────┘
```

**Dispenser subsystem** — owns the sequence counter. SQLite or file backing. Handles all concurrent callers via the Unix socket. No network involvement. Boring and reliable by design.

**Presence subsystem** — uses `libavahi-client` to announce what the local node is currently editing and discover what peers on the same LAN are working on. Surfaces pre-commit conflicts before git ever sees them: "Alice is also modifying FR-7-048 right now." Falls back gracefully when no peers are reachable. Works on a disconnected LAN without the mothership — directly relevant for air-gapped environments where Linear or Figma-style central WebSocket presence is unavailable.

Socket activation via systemd (Linux) or launchd (macOS) means the daemon starts on first client connection and shuts down when idle. Named pipes accomplish the same on Windows.

**Phase 1 ships without the daemon.** The file counter is the Phase 1 implementation. The daemon is a Phase 3 upgrade that slots in behind the stable `libaida-client` interface without any changes to callers.

---

## 5. Two-Tier ID Scheme

### 5.1 The Problem at Scale

The node-namespaced ID `FR-7-048` is unambiguous and immutable. But it carries a social cost at scale: "FR forty-eight" is ambiguous in verbal communication — node 7's FR-48 and node 11's FR-48 are different objects. Every successful issue and requirements tracking system in production converges on `{TYPE}-{INTEGER}`: Jira's `ENG-423`, Linear's `ENG-423`, GitHub's `#423`, Phabricator's `T1847`. Teams resist any scheme that makes IDs longer or less pronounceable. This is a social constraint, not a technical one, and social constraints kill adoption.

### 5.2 The Key Insight

The immutability constraint applies to IDs embedded in committed code or documentation **on the main branch**. A requirement existing only on a feature branch has not yet been embedded anywhere that will persist to trunk. The constraint applies at the point of trunk merge, not at the point of object creation.

### 5.3 Two-Tier Definition

**Tier 1 — Node ID (assigned at creation, always)**

```
FR-7-048
```

Assigned by the local dispenser at object creation. Immutable from that moment. The primary `id` field. Valid forever, in all contexts, on all branches. No network required.

**Tier 2 — Agreed ID (assigned at merge-to-trunk)**

```
FR-423
```

A short global sequential integer assigned at the moment a feature branch is merged to trunk. Stored as an additional `agreed_id` field. Assigned by CAS increment on a shared counter — the same mechanism as node registration. After assignment, both IDs permanently resolve to the same UUID. Neither is deprecated.

### 5.4 The Agreed ID Counter

```toml
# aida/registry/agreed_counters.toml
FR   = 422
FEAT = 89
TEST = 341
ARCH = 12
```

One counter per type, global across the entire workspace and all repos within it. The merge gate increments these counters using the same CAS push loop as node registration.

### 5.5 Merge Gate Procedure

```
git merge feature/auth-refactor
    ↓
aida merge-gate
    ↓
Pull latest aida repo
For each new object in the branch lacking an agreed_id:
    Read counter for this type  (e.g., FR = 422)
    Assign agreed_id = "FR-423"
    Increment counter to 423
    Write agreed_id into object TOML
Commit agreed ID assignments to aida repo
Push  (retry on rejection — same CAS loop as node registration)
    ↓
Merge to trunk proceeds
```

The merge gate is the only point where central coordination is required after initial node registration. Merging to trunk is inherently a connected operation — this is not a new constraint.

### 5.6 Usage by Context

| Context | Preferred ID | Reason |
|---|---|---|
| Feature branch (pre-merge) | `FR-7-048` | Agreed ID not yet assigned |
| Code comments on trunk | `FR-423` | Short, pronounceable, Jira-like |
| Cross-reference in any doc | Either | Both resolve to same UUID |
| Verbal standup reference | `FR-423` | Globally unambiguous |
| git blame / provenance | `FR-7-048` | Shows which node and when |

---

## 6. Multi-Repo Workspace Architecture

### 6.1 The Problem

If the aida database is shared across multiple code repos — `pacgate`, `pacinet`, `pacmate` — sequence numbers must be globally unique within the workspace, not per-repo. Otherwise node 7 creates `FR-7-001` in `pacgate` and `FR-7-001` in `pacinet`, which collide the moment they are joined in the shared database. The originating repo is metadata on the object, not a component of the ID.

### 6.2 Why Not a Submodule

Git submodules model a **versioned pinned dependency**. The aida repo is a **shared live workspace** that multiple code repos write to concurrently. These are opposite use cases. Submodules impose:

- Two-phase commits: every aida change requires a submodule pointer update in each code repo
- `git pull` does not automatically advance submodules
- Cloning requires `--recurse-submodules`
- CI/CD pipelines need explicit submodule awareness throughout

Do not use submodules for the aida repo.

### 6.3 Sibling Repo Convention

The aida repo lives alongside the code repos in a workspace directory. It has its own `.git` and is invisible to its siblings — git stops traversing at a `.git` boundary.

```
gdms-workspace/
  pacgate/              ← git repo, standalone
  pacinet/              ← git repo, standalone
  pacmate/              ← git repo, standalone
  aida/                 ← git repo, standalone — the shared workspace DB
  .aida-workspace       ← workspace config, not a git repo
```

```toml
# .aida-workspace
workspace  = "gdms-disruptive"
aida_path  = "./aida"
repos      = ["pacgate", "pacinet", "pacmate"]
```

Aida tooling resolves `.aida-workspace` by walking up the directory tree from the current working directory — the same convention cargo uses to find `Cargo.toml`. Any Aida command run inside any code repo automatically locates the aida repo without explicit per-session configuration.

### 6.4 Workspace Setup (One-Time Per Developer)

```bash
mkdir gdms-workspace && cd gdms-workspace
git clone git@gdms:pacgate.git
git clone git@gdms:pacinet.git
git clone git@gdms:pacmate.git
git clone git@gdms:aida.git
aida workspace init      # writes .aida-workspace, runs aida init for node registration
```

Adding a new code repo is just cloning it into the workspace directory. No submodule linkage to update anywhere.

### 6.5 Repo as Object Metadata

The originating code repo is a metadata field on the object, not part of the ID. Cross-repo relations work naturally — both objects live in the same aida repo with globally unique IDs.

```toml
# aida/objects/FR-7-048.toml
id          = "FR-7-048"
agreed_id   = "FR-423"
uuid        = "018e7f3a-..."
repo        = "pacgate"          # metadata for filtering — not addressing
created_by  = 102
created_on  = 7
```

```toml
[[relation]]
from  = "FR-7-048"       # pacgate requirement
to    = "FEAT-3-042"     # pacinet feature
type  = "constrains"
```

`aida list --repo pacgate` filters by the `repo` field. `aida show FR-423` resolves regardless of which repo the object originated from.

---

## 7. Storage Architecture

### 7.1 Layered Storage Model

| Layer | Technology | Role | Disposable? |
|---|---|---|---|
| Event Log | Git (aida repo) | Source of truth. Immutable history. | No |
| Object Files | TOML (aida/objects/) | Human-readable, git-diffable object state | No — committed |
| Local Read Model | SQLite (per clone) | Fast offline queries. Seeded from git on pull. | Yes — rebuild from git |
| Central Read Model | PostgreSQL (server) | Team-wide queries, dashboards, CI/CD gates | Yes — rebuild from git |

**Postgres is not the database. It is a cache of the database. The database is git.**

A corrupt or unavailable Postgres is a performance problem, not a data loss problem. The schema can evolve freely — migration is a rebuild from the log.

### 7.2 Repository Layout

```
aida/                             # standalone git repo
  objects/
    FR-7-001.toml                 # one file per object
    FR-7-002.toml
    FEAT-3-042.toml
  relations/
    FR-7-001.toml                 # relations keyed by source object
  registry/
    node_counter.toml             # shared sequential node counter
    agreed_counters.toml          # shared sequential agreed ID counters per type
    nodes.toml                    # append-only node registry
    users.toml                    # append-only user registry
```

One file per object is a critical design choice. Concurrent edits to different objects by different nodes never produce file-level conflicts. The only merge conflicts that surface are genuine — two users editing the same object simultaneously.

### 7.3 Object File Format

```toml
# aida/objects/FR-7-001.toml
id           = "FR-7-001"
agreed_id    = "FR-423"          # empty string until merge-to-trunk
uuid         = "018e7f3a-b4c2-7d00-8a1f-3e5c9f2a1b0d"
type         = "FR"
repo         = "pacgate"
created_by   = 102
created_on   = 7
created_at   = "2025-03-13T14:22:00Z"
modified_at  = "2025-03-13T15:01:00Z"
title        = "OAuth2 token validation"
status       = "draft"
priority     = "high"

[body]
text = """
The system shall validate OAuth2 bearer tokens on every authenticated
API endpoint using the configured JWKS endpoint.
"""

[meta]
tags    = ["auth", "security", "api"]
version = 3
```

### 7.4 Relations Format

Relations are append-only. Deletions are tombstones — never physical removes — ensuring the git log is a complete audit trail.

```toml
# aida/relations/FR-7-001.toml

[[relation]]
from       = "FR-7-001"
to         = "FEAT-3-042"
type       = "implements"
added_by   = 102
added_at   = "2025-03-13T14:22:00Z"

[[relation]]
from       = "FR-7-001"
to         = "FEAT-3-042"
type       = "implements"
deleted    = true
deleted_by = 102
deleted_at = "2025-03-15T09:00:00Z"
```

### 7.5 SQLite Locally, PostgreSQL Centrally

Each clone runs an embedded SQLite database seeded from the aida repo on pull, updated optimistically on local writes. The dispenser counter also lives here — separate tables, same file. PostgreSQL runs server-side for team-wide queries, full-text search, and CI/CD gates. Running PostgreSQL on every developer machine is unnecessary infrastructure overhead.

---

## 8. Synchronization Architecture

### 8.1 Write Path

1. Call `libaida-client` dispenser: `next(FR)` → `49`
2. Write `FR-7-049.toml` to local `aida/objects/`
3. Immediately upsert into local SQLite read model (optimistic)
4. UI reflects the new object with a sync-pending indicator
5. Background: `git commit` + `git push` to aida remote when network is available
6. Server sync service detects the push, updates central PostgreSQL
7. Other nodes see the new object on their next `git pull` of the aida repo

### 8.2 Read Path — Tiered

| State | Read Source | Latency | Notes |
|---|---|---|---|
| Connected, Postgres fresh | PostgreSQL | Sub-millisecond | Full relational query power |
| Connected, Postgres rebuilding | Last-known state | Sub-millisecond | Show staleness indicator |
| Disconnected, SQLite seeded | Local SQLite | Sub-millisecond | Fast, correct, possibly behind remote |
| Disconnected, no SQLite | TOML files direct | Tens of ms | Always correct, always available |

### 8.3 The Sync Service

A server-side sync service watches the aida git remote and projects state changes into PostgreSQL by diffing each push commit against its parent:

- **Added files:** parse TOML, upsert object into Postgres
- **Modified files:** parse TOML, detect concurrent conflict if merge commit, upsert or flag
- **Deleted files:** apply tombstone
- **After processing:** checkpoint sync cursor to commit SHA

Post-merge integrity validation runs here: relations point to live objects, dependency graphs are acyclic, required fields populated.

### 8.4 Git Merge Behavior

```
Node 7  creates: FR-7-048.toml, FR-7-049.toml
Node 11 creates: FR-11-031.toml, FEAT-11-009.toml

git merge result: zero conflicts — all files are distinct
```

The only merge conflicts that arise are genuine — two users editing the same object file simultaneously. Git's 3-way merge surfaces them for human resolution.

---

## 9. Real-Time Updates & Web UI Architecture

### 9.1 The Web UI as Primary Real-Time Client

The web UI is the interface most sensitive to distributed changes — it needs to reflect updates from other users as they happen, not on next page load or next git pull. Because the web UI routes to a **central server per aida database** regardless of platform, the presence and real-time update problem is solved there with standard web infrastructure rather than with mDNS/Avahi, which is a LAN-only native-client concern.

```
┌─────────────────────────────────────────────────────┐
│                  Central Aida Server                 │
│                (per aida database)                   │
│                                                      │
│  ┌──────────────┐   ┌──────────────┐                │
│  │  Sync        │   │  Real-Time   │                │
│  │  Service     │   │  Hub         │                │
│  │              │   │              │                │
│  │  git poll /  │   │  WebSocket   │                │
│  │  webhook     │──▶│  SSE broker  │                │
│  │              │   │              │                │
│  └──────┬───────┘   └──────┬───────┘                │
│         │                  │                         │
│         ▼                  ▼                         │
│      PostgreSQL         connected                    │
│      read model         web clients                  │
└─────────────────────────────────────────────────────┘
         ▲
    git push/pull
    (aida repo)
         ▲
  native clients
  (CLI, egui, daemon)
```

### 9.2 Server-Sent Events vs WebSockets

For the web UI, **SSE (Server-Sent Events)** is the recommended default over WebSockets:

- SSE is unidirectional (server → client) — exactly the right model for "push me updates when state changes"
- SSE works over standard HTTP/1.1 and HTTP/2, passes through proxies and firewalls without special handling
- Automatic reconnection is built into the browser EventSource API
- WebSockets are bidirectional — appropriate only if the web UI also needs to send real-time data back (e.g., collaborative cursor positions, live editing). Reserve WebSockets for that upgrade path.

```
GET /aida/stream?workspace=gdms-disruptive
Accept: text/event-stream

data: {"event":"object_created","id":"FR-7-049","agreed_id":"FR-424","title":"..."}
data: {"event":"object_modified","id":"FR-423","field":"status","value":"active"}
data: {"event":"conflict_flagged","id":"FR-7-048","field":"title"}
```

### 9.3 Event Types

| Event | Payload | Trigger |
|---|---|---|
| `object_created` | Full object summary | New TOML file committed and pushed |
| `object_modified` | Object ID + changed fields | Existing TOML file modified and pushed |
| `object_deleted` | Object ID | Tombstone committed |
| `relation_added` | from, to, type | Relation entry appended |
| `relation_deleted` | from, to, type | Relation tombstone committed |
| `conflict_flagged` | Object ID + field | Merge produced concurrent MV-Register versions |
| `agreed_id_assigned` | Node ID form + agreed ID | Merge gate completed |
| `sync_cursor` | Commit SHA + timestamp | Heartbeat — clients know how fresh their view is |

### 9.4 Presence via the Central Server

Because the web UI routes through the central server, **user presence** (who is viewing or editing what) is managed server-side rather than via mDNS:

```
Client opens /aida/stream → server records presence: {user: 102, viewing: "FR-423"}
Client sends  POST /aida/presence with {editing: "FR-7-048"}
Server broadcasts to all connected clients: {user: "Joe", editing: "FR-7-048"}
Client closes connection → server clears presence record (with TTL fallback)
```

This gives the web UI the same "Alice is editing FR-7-048 right now" capability that the Avahi presence subsystem provides for native clients on a LAN — but via the central server, working across any network topology including VPNs and remote workers.

### 9.5 Platform-Specific Presence: Native Clients

For **native clients** (CLI, egui desktop app, daemon), the presence mechanism is tiered by platform:

| Platform | Mechanism | Notes |
|---|---|---|
| Linux | Avahi / mDNS | Zero-config, LAN-local, works without server |
| macOS | dns-sd / Bonjour | Native Apple mDNS stack; same API shape as Avahi |
| Windows | Central server WebSocket | See Section 9.6 |
| Air-gapped LAN (any OS) | Avahi / mDNS | Server unreachable; LAN presence still works on Linux/macOS |

### 9.6 Windows Presence Recommendation

**Recommendation: use the central server WebSocket channel as the Windows presence mechanism, not mDNS.**

Windows mDNS support is fragmented. Windows 10 1703+ includes a limited mDNS responder, but it is not exposed via a stable developer API and third-party mDNS libraries on Windows are unreliable. The `mdns` Rust crate has Windows support but it is not at parity with the Linux/macOS implementations and has known reliability issues.

Since Windows native clients in a GDMS environment will almost always be on a network with access to the central server (VPN, corporate LAN), the central server WebSocket channel provides better-than-mDNS presence coverage anyway — it works across subnets, includes remote workers, and requires no per-machine mDNS infrastructure.

The `libaida-client` presence API is the same on all platforms:

```rust
// Same interface everywhere — implementation differs per platform
fn announce(editing: ObjectId) -> Result<()>
fn discover() -> Result<Vec<PeerPresence>>
fn on_conflict(callback: impl Fn(ConflictNotice)) -> Result<()>
```

On Linux/macOS the backing implementation uses Avahi/dns-sd. On Windows it uses the central server WebSocket. On an air-gapped network where the server is unreachable, Windows falls back gracefully to no presence (the dispenser continues working — presence is never a dependency of ID generation).

### 9.7 Sync Freshness Indicator

The web UI should always display a sync freshness indicator driven by the `sync_cursor` heartbeat event. Users need to know whether what they are looking at reflects the last push from two minutes ago or two days ago.

```
● Live  —  synced 12 seconds ago  (commit a3f8c2d)
◐ Stale —  last synced 4 min ago  (commit a3f8c2d)  [Refresh]
○ Offline — cannot reach server
```

The PostgreSQL read model's cursor checkpoint (Section 8.3) drives this indicator directly.

---

## 10. CRDT & Conflict Resolution Strategy

### 10.1 Theoretical Foundation

CRDTs (Conflict-free Replicated Data Types) guarantee convergence to identical state on any two nodes that have received the same set of updates, regardless of delivery order — **strong eventual consistency (SEC)**.

The foundation is a join-semilattice with merge operation (`⊔`) that is commutative, associative, and idempotent:

```
a ⊔ b = b ⊔ a              (order of receipt doesn't matter)
(a ⊔ b) ⊔ c = a ⊔ (b ⊔ c)  (batching doesn't matter)
a ⊔ a = a                   (receiving twice is safe)
```

### 10.2 CRDT Type Assignments Per Field

| Field / Structure | CRDT Type | Behavior on Conflict |
|---|---|---|
| Object collection (project) | OR-Set | Union of adds; removes carry tokens; re-add after delete is safe |
| `status`, `priority` (enums) | LWW-Register | Last write wins by HLC; concurrent loser silently discarded |
| `title`, `description` | MV-Register | All concurrent versions preserved; flagged for human resolution |
| Body text (collaborative) | RGA Sequence | Character-level CRDT; concurrent edits merge without loss |
| `tags`, `links` (sets) | OR-Set | Union merge; deletions are token-based |
| Relations | Append-only log | Tombstone-based; always merges cleanly |
| Counts, metrics | PN-Counter | Merge by summing per-node increment/decrement counters |

LWW is acceptable for state machine fields where losing a concurrent write is visible and recoverable. It is unacceptable for text fields where silent overwrite destroys work.

### 10.3 Hybrid Logical Clocks

```
HLC = max(wall_clock, last_known_hlc) + logical_counter
```

HLC stays close to wall time while preserving causality and handling clock skew. HLC replaces system time throughout Aida in all LWW decisions and causality determinations from Phase 2 onward.

### 10.4 Vector Clocks for Concurrency Detection

```
VC(node 7)  = {7: 3, 11: 1, 14: 0}
VC(node 11) = {7: 2, 11: 4, 14: 1}

7 before 11?  No — 7:3 > 7:2
11 before 7?  No — 11:4 > 11:1
Result: CONCURRENT — flag for human resolution
```

### 10.5 Where CRDTs Do Not Help

- **Meaning-level conflicts:** CRDTs merge text without data loss; the result may be semantically contradictory. Require human sign-off before publishing to trunk.
- **Invariant violations:** Sprint budgets, acyclic graph constraints. Enforce as post-merge validation gates, not at write time.
- **Cascading deletes:** Structural deletion is CRDT-safe; reference cleanup across the graph is a post-merge integrity check.

CRDTs ensure you never lose data. Post-merge validation ensures semantic integrity. Human review ensures meaning-level conflicts are resolved before becoming agreed truth.

---

## 11. Implementation Phasing

| Phase | Deliverable | Key Decisions |
|---|---|---|
| Phase 1 | Node ID via git CAS. File-based dispenser with lockfile. UUID canonical identity. Node-namespaced display IDs. Append-only op-log. LWW on all fields. Sync via git push/pull on sibling aida repo. Central server with PostgreSQL read model (webhook + 60s poll, 30s SLA). SSE endpoint for web UI. SQLite full rebuild only. | The op-log is the most important investment. Accept that concurrent same-field edits lose one version — log the loss. SSE from day one at minimal cost. |
| Phase 2 | MV-Register on `title` and `description`. Conflict UI — accept-mine / accept-theirs only (simplest first, no diff view). HLC timestamps. Two-tier agreed IDs at merge-to-trunk. Server-side presence for web clients. | First-class conflict surfaces with explicit human resolution required. No auto-resolution ever. Short agreed IDs for trunk references. |
| Phase 3 | OR-Set for object collection. Vector clocks. Delta-sync. SQLite local read model; full rebuild remains default, incremental added as `aida.seeding = "full" | "incremental"` config option. `aida-daemon` with Unix socket. Avahi presence on Linux/macOS. Central server WebSocket presence for Windows. Side-by-side diff view in conflict UI. | Full CRDT correctness. Daemon handles concurrent native callers. LAN-local conflict detection before git sees it on Linux/macOS. |
| Phase 4 | Evaluate Loro or Automerge as underlying CRDT engine. Migrate domain model on top. Inline merge editor for body text conflicts. WebSockets added alongside SSE only if collaborative live editing is explicitly scoped. | Loro (Rust-native) is the leading candidate. WebSocket upgrade is conditional and additive — SSE is never replaced. |

---

## 12. Technology Decisions

### CRDT Libraries

| Library | Language | Strengths | Recommendation |
|---|---|---|---|
| Loro | Rust / WASM | Rich CRDT set, time-travel, compact binary, actively maintained | Primary Phase 4 candidate |
| Automerge | Rust / JS | Battle-tested, good Rust port, document model | Alternative if Loro proves immature |
| diamond-types | Rust | Highest-performance sequence CRDT | Consider for body text specifically |
| Yjs | JS | Best-in-class collaborative text | Frontend only, if a web UI is added |

### Storage & Infrastructure Stack

| Component | Technology | Rationale |
|---|---|---|
| Object store | TOML files in git | Human-readable, git-diffable, merge-friendly |
| Dispenser + local read model | SQLite | Embedded, zero infrastructure, file-per-clone |
| Central read model | PostgreSQL | Full relational power, concurrent connections, full-text search |
| Canonical identity | UUID v7 | Time-ordered, no coordination, universally supported |
| Timestamps | Hybrid Logical Clock | Causality + wall time, handles clock skew |
| Web UI real-time updates | SSE (Server-Sent Events) | Unidirectional push, works over HTTP/1.1+HTTP/2, auto-reconnect |
| Web UI presence | Central server HTTP | User presence tracked server-side, broadcast via SSE |
| Native LAN presence (Linux/macOS) | Avahi / dns-sd (mDNS) | Zero-config, works without server, air-gap friendly |
| Native presence (Windows) | Central server WebSocket | mDNS unreliable on Windows; server channel superior coverage |
| Daemon IPC | Unix domain socket | Microsecond latency, standard on all target platforms |

---

## 13. Type Prefix Vocabulary

| Prefix | Object Type | Node ID Form | Agreed ID Form |
|---|---|---|---|
| `FR` | Functional Requirement | `FR-7-001` | `FR-423` |
| `NFR` | Non-Functional Requirement | `NFR-7-005` | `NFR-44` |
| `FEAT` | Feature / Epic | `FEAT-3-042` | `FEAT-89` |
| `TASK` | Work Item / Task | `TASK-7-103` | `TASK-512` |
| `TEST` | Test Case | `TEST-14-007` | `TEST-341` |
| `FILE` | Source File Reference | `FILE-7-017` | — |
| `FUNC` | Function / Symbol Reference | `FUNC-7-204` | — |
| `DOC` | Document Reference | `DOC-3-011` | — |
| `RISK` | Risk Item | `RISK-7-003` | `RISK-18` |
| `ARCH` | Architecture Decision Record | `ARCH-7-002` | `ARCH-12` |

---

## 14. Open Questions

All questions from prior versions have been resolved. The following represent the only remaining open decisions.

- **Cross-workspace references:** If `FILE-7-017` refers to an object in a different workspace entirely, how are cross-workspace UUIDs resolved and validated at the tooling level? This requires a workspace federation design that is out of scope until multi-workspace usage is observed in practice.

---

### Resolved Decisions (archived for record)

**SQLite seeding strategy** *(resolved v0.5)*
Full rebuild from git on every pull is the only supported mode in Phase 1 and Phase 2. Incremental delta application is deferred to Phase 3. Full rebuild is always correct, is the simplest implementation, and is fast enough for any realistic object count at early team scale. A project-level configuration option (`aida.seeding = "full" | "incremental"`) will be added in Phase 3 when incremental is implemented, defaulting to `"full"`.

**Conflict UI resolution approach** *(resolved v0.5)*
Multiple resolution approaches are supported, implemented in order of simplicity:
- Phase 2: Accept-mine / accept-theirs buttons only. No diff view. Fast to implement, covers the majority of cases.
- Phase 3: Side-by-side diff view with field-level highlighting.
- Phase 4: Inline merge editor for body text conflicts, if collaborative editing is scoped.
The UI never auto-resolves a semantic conflict. All resolution requires an explicit human action.

**PostgreSQL rebuild SLA** *(resolved v0.5)*
Target: < 30 seconds from push to Postgres reflecting it during active hours. Implementation: webhook-driven sync (instant trigger on push) with a 60-second polling fallback in case a webhook is missed. Force a full rebuild when either of the following thresholds is breached: (a) the sync cursor has fallen more than 50 commits behind, or (b) staleness exceeds 1 hour wall-clock time. The 1-hour threshold means overnight gaps with no pushes are acceptable; anything resembling a stuck sync triggers a rebuild automatically. The `sync_cursor` SSE heartbeat event makes current staleness transparent to web UI users at all times.

**Agreed ID for objects never merged to trunk** *(resolved v0.5)*
Objects that are never merged to trunk — scratch work, exploratory requirements, abandoned branches — never receive an agreed ID. This is semantically correct: the agreed ID signals "reviewed and ratified via the merge process." An object that never completes that process simply remains identified by its node-namespaced ID and UUID for its lifetime. This is acceptable indefinitely. No cleanup or forced ratification policy is required.

**SSE vs WebSocket upgrade path** *(resolved v0.5)*
SSE is the permanent default for all unidirectional push updates (object changes, presence, sync cursor). WebSockets are introduced only if Phase 4 collaborative live body-text editing is scoped and requires bidirectional real-time communication. The trigger condition is explicit: if `aida edit` on the web UI becomes a collaborative live session (multiple cursors, character-level sync via RGA), WebSockets are added as an additional transport alongside SSE. SSE is not replaced — it remains the transport for all non-collaborative update events. The migration is additive, not a replacement.

**Stale description audit frequency** *(resolved v0.5)*
For projects using Mode B (ID with inline description), `aida audit --stale-descriptions` is a manual job. No CI gate, no pre-commit hook, no scheduled automation in Phase 1 or Phase 2. Engineers run it when descriptions feel stale. The output is a list of diverged descriptions with suggested updates; all changes require explicit human approval before being written. Automation of this audit is a future enhancement if teams find manual runs insufficient.

---

## 15. Gotchas & Design Traps

These are failure modes, non-obvious constraints, and design traps identified during architecture development. Each represents a decision that has already been made — re-litigating requires a strong argument.

---

### G-01 — Renumbering is a correctness violation, not a maintenance issue

Once an ID appears in a committed source file or document on the main branch, it is immutable. There is no such thing as a "safe rename." `git blame` traces authorship through IDs. Any scheme that involves provisional IDs replaced after sync is incompatible with this constraint.

---

### G-02 — The hyphen separators are structural, not decorative

Both hyphens in `{TYPE}-{NODEID}-{SEQ}` are mandatory. Without them, `FR-111001` is ambiguous. With them, `FR-11-001` and `FR-1-1001` are unambiguous regardless of digit count. There is no hard limit on either field's length.

---

### G-03 — User ID and Node ID serve different purposes; do not conflate them

User ID is attribution ("who"). Node ID is addressing ("where born"). A user with three clones has three node IDs. Encoding user ID into the object ID fails the moment the same user initializes a second clone.

---

### G-04 — Random node IDs require a separate validation step; sequential integers do not

Sequential integers assigned via the git CAS push loop are unique by construction — push success is proof. A random ID requires a separate registry check. Use sequential integers.

---

### G-05 — The CAS retry loop must pull before retrying, not just retry the push

On push rejection: `git pull --rebase`, re-read the counter, claim the new value, commit, push again. Retrying the push without pulling will always fail. The pull is not optional in the retry loop.

---

### G-06 — Offline node registration is not supported; this is a hard rule

`aida init` requires network access. A clone that cannot complete the CAS push loop cannot create objects. There is no provisional state. After successful registration, the clone is fully autonomous offline indefinitely.

---

### G-07 — The dispenser is a local problem; treat it as one

Once a Node ID is assigned, sequence number generation requires no network, no coordination, no CAS loop. It is a monotonic counter. Do not add distributed coordination to the dispenser. If that urge arises, the problem belongs in the node registration layer.

---

### G-08 — Postgres is a cache, not the database; treat it accordingly

PostgreSQL holds a materialized projection of the git log. It can be wiped and rebuilt at any time. Do not build features that have no git-layer equivalent. Schema migration is a rebuild, not an ALTER TABLE.

---

### G-09 — One file per object is load-bearing, not a style preference

A monolithic object file produces merge conflicts on every concurrent edit to unrelated objects. One file per object ensures only genuine conflicts surface, and git's 3-way merge can handle them.

---

### G-10 — Inline descriptions in code are a project configuration choice, not a correctness issue

The ID embedded in code is always immutable and authoritative. Whether an inline description accompanies it is a project-level configuration option with a documented trade-off:

- **ID only** (`/// [FR-7-048]`): always current, requires tooling lookup to read the title
- **ID with description** (`/// [FR-7-048] OAuth2 token validation`): convenient when reading code without tooling, but may become stale if the requirement title changes

Projects that embed descriptions accept the staleness risk explicitly. The description is informational — the ID is the truth. Run `aida audit --stale-descriptions` periodically to identify diverged inline descriptions. Updates are cosmetic (ID unchanged), clean to diff, and safe to apply in bulk.

---

### G-11 — Relations must be append-only; deletions are tombstones

Physically removing a relation entry creates false history — it looks as though the relation never existed. Use tombstone entries. The git log is the audit trail. Never rewrite it.

---

### G-12 — LWW silently discards concurrent writes; unacceptable for text fields

LWW is appropriate for state machine fields. It is never appropriate for `title`, `description`, or body text. Use MV-Register — keep all concurrent versions, surface the conflict — for any field where silent discard is unacceptable.

---

### G-13 — CRDTs handle structure; they do not resolve meaning

Two engineers can both edit an acceptance criterion offline. The CRDT merges the text without data loss. The merged result may be semantically contradictory. Converged state is not the same as agreed state.

---

### G-14 — Wall clocks are unreliable across nodes; use HLC everywhere

`SystemTime::now()` for LWW timestamps causes incorrect merge decisions under clock skew. HLC stays close to wall time while preserving causality. HLC is the universal timestamp throughout Aida from Phase 2 onward.

---

### G-15 — Block pre-allocation (HiLo pattern) solves the wrong problem

Pre-allocated number blocks introduce new failure modes: blocks run out mid-session, unused numbers create permanent gaps, block size has no correct value. Node-namespaced IDs eliminate the coordination problem entirely. Do not revisit block pre-allocation.

---

### G-16 — Cascading deletes are not a CRDT problem

Structural deletion (tombstone in OR-Set) is CRDT-safe. Removing all references to a deleted object across the graph is not. Cascading reference cleanup is a post-merge integrity step, not an atomic operation. Attempting atomicity leads to the distributed transaction problem.

---

### G-17 — Do not use submodules for the aida repo

Submodules model a versioned pinned dependency. The aida repo is a shared live workspace multiple code repos write to concurrently. These are opposite use cases. Use the sibling repo convention.

---

### G-18 — SQLite seeding must be idempotent and atomic

All upserts into the SQLite read model must be idempotent (insert-or-replace keyed on UUID). The rebuild must be atomic — fully committed or fully rolled back. The rebuild cursor must be stored inside the same SQLite transaction. An interrupted rebuild must self-correct on the next run.

---

### G-19 — UUID v7, not v4

UUID v4 is purely random. UUID v7 is time-ordered. For a system where objects are created sequentially and queried by recency, v7 provides significantly better index locality and sort performance. Default to v7 everywhere.

---

### G-20 — The git merge conflict surface is intentional; do not suppress it

When two users edit the same object file concurrently, git produces a merge conflict. This is correct — it signals a genuine semantic conflict requiring human judgment. Surface it in the Aida UI. Do not implement an automatic merge strategy that silently picks a winner.

---

### G-21 — The sync service rebuild must be authoritative, not additive

The sync service must process the full git log in order to arrive at canonical state — not apply recent diffs on top of potentially stale state. An additive sync drifts from true state wherever tombstones, field overwrites, or merge resolutions are involved. The rebuild cursor must be checkpointed and resumable from the last confirmed commit SHA.

---

### G-22 — The agreed ID counter is per-type, not global

A single global counter produces `FR-1`, `FEAT-2`, `TEST-3` — nonsensical across types. Per-type counters produce `FR-423`, `FEAT-89`, `TEST-341` — each independently compact and meaningful. One key per type prefix in `agreed_counters.toml`.

---

### G-23 — The daemon's presence subsystem must degrade gracefully

When no peers are reachable — disconnected laptop, air-gapped network, Avahi unavailable — the dispenser subsystem must continue operating normally. Presence is an enhancement layered on top of the daemon, not a dependency of it. The daemon must not fail to start or fail to dispense IDs when mDNS is unavailable.

---

### G-24 — Sequence numbers are global per node across all repos; do not re-scope them per repo

Node 7's FR counter is a single monotonic integer across the entire workspace. Scoping per-repo reintroduces the collision problem: `FR-7-001` in `pacgate` and `FR-7-001` in `pacinet` collide in the shared database. The originating repo is metadata on the object. It is not a component of the ID.

### G-25 — On Windows, use the central server for presence; do not rely on mDNS

Windows mDNS support is fragmented and not exposed via a stable developer API. Do not build the Windows presence path on mDNS or third-party mDNS libraries. Windows native clients use the central server WebSocket channel for presence — which provides superior coverage anyway (works across subnets and VPNs, not just the local LAN segment). The `libaida-client` presence API is identical on all platforms; only the backing implementation differs.

---

### G-26 — Presence must never be a dependency of the dispenser

The presence subsystem (Avahi on Linux/macOS, central server WebSocket on Windows) announces what a node is editing and discovers peer conflicts. The dispenser subsystem generates sequence numbers. These must remain strictly independent. If the presence subsystem fails to connect, the daemon must continue dispensing IDs normally. A developer on an air-gapped machine with no peers and no server access must be able to create objects without degradation. Presence is an enhancement; dispensing is infrastructure.

---

*End of document.*
