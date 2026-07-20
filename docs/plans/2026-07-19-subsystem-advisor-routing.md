# First-class subsystems and specialized advisor routing

Date: 2026-07-19
Specs: TASK-0434, TASK-0435, TASK-0436, TASK-0437
Status: design proposal

This plan turns the strategy cluster into one coherent build shape. It is not
an implementation patch. The proposed contract is:

- represent subsystems as durable project configuration, not as requirements;
- compute subsystem membership deterministically from spec metadata and file
  paths;
- route advisor/reviewer work to scoped roles when the scope is unambiguous;
- keep the master advisor as the arbitration and safety authority;
- keep universal context always loaded, then add focused subsystem context.

<!-- trace:TASK-0434 | ai:codex -->
<!-- trace:TASK-0435 | ai:codex -->
<!-- trace:TASK-0436 | ai:codex -->
<!-- trace:TASK-0437 | ai:codex -->

## Current State

AIDA already has the ingredients, but not one first-class model:

- `docs/multi-advisor-coordination.md` defines SPIKE-10 Track A as focused
  memories and later subsystem-scoped advisors.
- `docs/architecture/autonomy-and-escalation.md` defines cold-boot advisor,
  fork-from-live advisor, conservative Type A/B/C calibration, and master
  human escalation.
- `docs/aida/discipline/docs-lane.md` proves a single-writer scope lane by
  convention for docs work.
- Specs already use `feature`, tags, parent relationships, queues, roles, and
  local leases, but those are separate signals.

The missing piece is a durable subsystem registry that all of those surfaces can
read.

## Proposed Model

Add project-local subsystem definitions under the git-canonical store as
configuration objects. Do not model subsystems as requirement nodes: they are
classification and routing policy, not work items.

Initial physical shape:

```yaml
# .aida-store/registry/subsystems.yaml
version: 1
subsystems:
  - name: autonomy
    title: Autonomy and escalation
    description: Orchestrator, drain, punt, advisor-tier, and mode semantics.
    advisor_role: advisor:autonomy
    reviewer_role: reviewer:autonomy
    feature_matches: ["agents"]
    tag_matches: ["autonomy", "orchestrator", "advisor-autopilot"]
    path_globs:
      - "aida-cli/src/auto_complete.rs"
      - "aida-cli/src/punt.rs"
      - "docs/architecture/autonomy-and-escalation.md"
    memories: ["autonomy", "orchestrator"]
    queue: implementer
    risk_policy: keystone
    escalation_policy: master-advisor
```

Fields:

| Field | Meaning |
|---|---|
| `name` | Stable lowercase slug. This is the ID used by config and role suffixes. |
| `title` | Human-readable label. |
| `description` | Short scope description for prompts and status surfaces. |
| `advisor_role` | Preferred advisor role, e.g. `advisor:mcp`. |
| `reviewer_role` | Optional specialized reviewer role. |
| `feature_matches` | Requirement `feature` values that imply membership. |
| `tag_matches` | Requirement tags that imply membership. |
| `path_globs` | Files whose edits imply membership. |
| `memories` | Focus keys for memory/context loading. |
| `queue` | Default queue lane when this subsystem is the sole owner. |
| `risk_policy` | `routine`, `guided`, `keystone`, or `security`. |
| `escalation_policy` | `local-advisor`, `master-advisor`, or `human-gated`. |

The registry should be parsed into cache tables for fast search, but the YAML
file remains canonical.

## Membership

Compute subsystem membership from three ordered signals:

1. Explicit spec tags: `subsystem:<name>` always wins.
2. Requirement metadata: `feature` and ordinary tags match registry rules.
3. Touched paths: active branch diff, trace comments, or plan critical files
   match `path_globs`.

The result is a set, not a scalar. A spec can touch multiple subsystems. A file
can belong to multiple subsystems.

Tie rules:

- one subsystem -> route to its scoped roles;
- multiple subsystems with same risk/escalation policy -> include all focused
  contexts and choose the least-surprising role by primary path ownership;
- multiple subsystems with different policies -> master advisor arbitrates;
- no subsystem -> master advisor, universal context only.

Store computed membership in the read cache only. Persist only explicit
`subsystem:<name>` tags or registry changes.

## Initial AIDA Subsystems

Seed these examples in docs/tests first, not as hardcoded production defaults:

| Name | Primary signals |
|---|---|
| `autonomy` | `auto_complete`, punt/advisor, zen/no-human, `docs/architecture/autonomy-and-escalation.md` |
| `orchestrator` | queue drain, sessions, worktree, leases, CI/PR phases |
| `mcp-server` | `mcp.rs`, MCP docs, tool schema contracts |
| `cli` | command parsing, CLI manual, user-facing terminal output |
| `tui` | terminal UI, launcher, cockpit, palette, status rendering |
| `docs` | `docs/**`, doc lane, generated manuals |
| `product` | intake, roadmap, positioning, feature shaping |
| `security` | sandbox, scrub, secrets, permissions, remote admin |

## Routing

Subsystem routing applies to advisor questions, punts, findings, reviews, and
queue selection.

| Input | Routing rule |
|---|---|
| Spec has one subsystem | Use `advisor:<subsystem>` if registered, else master advisor. |
| Spec has several subsystems | Prefer master advisor unless all involved policies are `routine` and one subsystem owns most changed files. |
| Spec has none | Master advisor. |
| Finding is path-scoped | Route from file membership. |
| Finding is spec-scoped | Route from spec membership. |
| Queue item has sole subsystem | Prefer subsystem queue/role if configured. |
| Review touches MCP contract plus CLI output | Master advisor: cross-contract and user-facing output. |
| TUI workflow changes queue semantics | Master advisor: UI plus lifecycle semantics. |
| Security change affects all agents | Human-gated or master advisor, never local-only. |

Role discovery should be conventional: any role name matching
`advisor:<subsystem>` or `reviewer:<subsystem>` is a scoped seat. Absence is not
an error; it falls back to the master advisor and records why.

Every routed answer must record:

- selected advisor/reviewer role;
- candidate subsystem set;
- rule that selected the role;
- fallback reason, when a scoped role was absent;
- arbitration reason, when master advisor overrode local scope.

## Context Loading

Context loading uses subsystem focus as a filter, not a replacement.

Always load:

- AGENTS/CLAUDE orientation;
- universal discipline docs;
- owning spec, parent, relationships, comments, and decision requests;
- lifecycle/trace/commit rules.

Load when focused:

- subsystem registry row;
- matching memory files or memory frontmatter tags;
- subsystem docs and plans;
- recent specs and findings in the subsystem;
- helper discovery limited to relevant command families.

Display focus in operator-facing status surfaces:

- CLI: `focus: autonomy,mcp-server` in queue/session/status summaries;
- TUI: a compact focus badge and filterable subsystem column;
- session manifest: `subsystems = [...]`, `advisor_role = ...`,
  `routing_reason = ...`.

Agents infer focus in this order:

1. explicit session/queue focus;
2. explicit `subsystem:<name>` tag;
3. spec feature/tag match;
4. worktree branch diff path match;
5. no focus.

## Arbitration And Authority

Subsystem advisors decide locally only when all are true:

- the spec is in one subsystem or the affected subsystems share compatible
  policy;
- the choice is Type A or recorded-B under the existing advisor calibration;
- the action is reversible or explicitly delegated to that subsystem;
- the registry risk policy is not `keystone` or `security`.

Escalate to the master advisor when:

- a decision changes file formats, MCP contracts, orchestrator semantics,
  lifecycle vocabulary, lease/session semantics, or cross-agent policy;
- two subsystem advisors disagree;
- the selected subsystem is absent or stale;
- the decision changes another subsystem's invariants;
- the routing explanation cannot cite a deterministic rule.

Escalate to the human when:

- master advisor authority is insufficient under existing policy;
- the choice is strategic, irreversible, security-sensitive, or unrecorded-B/C;
- a pending decision request or recorded human gate already exists.

ADR-14's asymmetric override rule applies to subsystem scope: an operator can
tighten from local advisor to master/human in one command; loosening from
master/human to local advisor requires an explicit force-style action and must
not persist silently.

## Migration

1. Land docs and proposed ADRs.
2. Add read-only registry parsing plus validation.
3. Seed examples for AIDA subsystems in tests/fixtures.
4. Add cache projection and `aida subsystem list/show/explain` read surfaces.
5. Wire memory/context focus, still read-only for routing.
6. Wire advisor/reviewer routing behind dry-run/explain output.
7. Enable scoped queues/roles only after routing explanations are stable.

Backfill existing specs only by explicit tags when useful. Do not mass-edit the
store from inferred memberships.

## Acceptance Mapping

- TASK-0434: registry shape, fields, membership algorithm, migration, examples.
- TASK-0435: routing rules, role naming, fallback, headless advisor selection,
  audit trail.
- TASK-0436: memory/docs/graph/helper context model, universal context rule,
  CLI/TUI/session focus indicators, inference order.
- TASK-0437: local-vs-master authority, conflict handling, keystone/security
  defaults, narrowed autopilot authority, examples.

## Open Build Questions

- Should the registry live at `.aida-store/registry/subsystems.yaml` or under a
  future project config namespace?
- Should `risk_policy` reuse `execution_mode` values directly or stay a
  subsystem-level policy that maps into execution modes?
- Should scoped role registration be explicit or inferred from active sessions?
- Which CLI command owns the first read-only surface: `aida subsystem` or
  `aida focus`?
