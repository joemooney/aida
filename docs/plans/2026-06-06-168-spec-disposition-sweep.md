# 168-spec disposition sweep (59 draft + 109 approved)

- **Date:** 2026-06-06 · **Driver:** advisor (autonomous /loop) · **Directive:** "All 168 must be resolved or dispositioned as requiring human input; if human input needed, the spec must carry the questions + possible answers. Do not pause until all 168 processed."
- **Constraint:** serial `aida` writes only (BUG-455 cache-lock — no parallel store writes).
- **Conventions:**
  - **Reject:** obsolete/stale/superseded/dup/test-fixture → `aida edit <id> --status rejected` + reason comment.
  - **Archive:** stale-but-not-wrong, keep audit trail.
  - **Needs-human-input:** tag `needs-human-input` + a `🔵 DECISION REQUEST` comment (questions + candidate answers a/b/c + recommendation).
  - **Ready:** well-formed implementable; comment `✅ READY — no human input needed` so the operator can batch-drain.
  - **Resolve:** implement/complete trivially-doable now.

## ✅ COMPLETE — all 168 dispositioned (2026-06-06)

| Disposition | Count | How to find |
|-------------|-------|-------------|
| Rejected | 39 | `aida list --status rejected` (stale auto-drafts, merged-PR reviews, test fixtures, OBE Claude-Code research spikes, obsolete-architecture specs) |
| Completed | 16 | already-implemented/shipped, adopted constraints/terms |
| Archived | 7 | parked & recoverable (`aida unarchive`) — low-value polish + marketing-deferred spikes |
| **Ready-to-implement** | **44** | `aida list --tags ready-to-implement` — bounded, no decision needed, queue-able |
| **Needs-human-input** | **62** | `aida list --tags needs-human-input` — each carries a `🔵 DECISION REQUEST` comment (questions + candidate options a/b/c + advisor recommendation) |

**Operator next step:** review the 62 `needs-human-input` packets (`aida list --tags needs-human-input`, then `aida show <ID>` for the decision request). The 44 `ready-to-implement` specs can be drained directly.

Method: per-spec read+reason parallelized via read-only triage subagents (8 total over draft+approved), dispositions written to durable files under `.aida/disposition-sweep/`, applied SERIALLY by the advisor (cache-lock-safe). Reject-vs-complete judgment applied per spec by the advisor (already-implemented → completed, not rejected).

## Progress

| Batch | Scope | Count | Status |
|-------|-------|-------|--------|
| 1 | Reject: auto-drafted drain-failure bugs + merged-PR review stories + test fixtures | 25 | ✅ done (pushed) |
| 1b | Complete stateless recorded facts: CON-1, TERM-1 | 2 | ✅ done (pushed) |
| 2 | Draft: strategic/vision → decision packets (STORY-519/520/522/523, EPIC-30/32/36, TASK-661/667/670, ...) | | pending |
| 3 | Draft: old specs (EPIC-7/8, FR-103/195/265/267) → verify→reject/ready/human | | pending |
| 4 | Approved: EPIC clusters (24/25/27/28/29/31/34/35/37/38) → decision packets / ready | | pending |
| 5 | Approved: ready bounded work → READY notes / resolve | | pending |
| 6 | Approved: spikes (8/13/14/15/18-22/38/40/47/48/50) → defer/decision/reject | | pending |

**Progress: 66 / 168 dispositioned (39%).** DRAFT side 100% done (16 remain, all needs-human-input flagged). Remaining: 102 approved.

### Batch 2 done (38 draft + EPIC-29) — via 2 read-only triage subagents, applied serially
- Rejected (6): EPIC-8 (superseded by plan tooling), EPIC-7 (predates git-canonical), TASK-546 (deferred tickler), TASK-656 (empty placeholder), FR-195 (already impl: scaffold-status --report), FR-103 (already impl: current_user_id + git config).
- Archived (5): TASK-655/657/658/659 (STORY-446 Low polish, no demand), TASK-139 (doctor/recover merge, deferred per own recurrence-gate).
- Ready→approved (11): EPIC-36, TASK-670/667/661/649/579/125, BUG-417/415, FR-265/267 (tag `ready-to-implement`).
- Needs-human-input packets (16 draft + EPIC-29): STORY-523/522/520/519/265/262, EPIC-30/32/29, TASK-255/130, STORY-267/268/269/270/266/499.

**Lesson:** `aida edit --tags` REPLACES (clobbers) — use `--add-tag`. (Clobbered 11 ready specs' tags, restored from JSON dump.)

### Conventions confirmed
- Ready → status approved + tag `ready-to-implement`.
- Needs-human → tag `needs-human-input` + `🔵 DECISION REQUEST` comment (status stays). Query: `aida list --tags needs-human-input`.
- Archived parked items keep status but hidden from default view.

### Remaining: 102 approved — next batches
EPIC clusters (24/25/27/28/31/33/34/35/37/38 + EPIC-22), the EPIC-35 GitLab story chain (509/510/511/512), MCP modernization (STORY-82/399/401/82, EPIC-27), status surfaces (456/457/539/662), spikes (8/13/14/15/18-22/38/40/47/48/50), multi-agent/registry (425/452/469/416/410/408/363/362, EPIC-31), git-verb (97/98/99/100/114/115), older stories (47/49/50/92/93/127/121), bugs (445/455), tasks (224/256/262/297/298/305/309/311/340/396/402/405/453/480/516/562/578/618/619/627/630/631/634/635/679), etc.

## Batch 1 — verified stale (reject)

Auto-complete-failure bugs (every referenced task Completed/Rejected/deferred):
BUG-461(TASK-457 deferred→BUG-462), 458(225✓), 457(671✓), 456(304✓), 454(673✓), 452/451/450(673✓ ×3 dup), 441(643✗), 439(642✗), 437(640✗), 435(639✗), 419(135✓)

Review-PR stories (all PRs merged/closed):
draft: STORY-526(PR-548 merged), 525(547), 524(546)
approved: STORY-500(450 closed), 485(347), 468(309), 466(307), 461(296), 458(293)

Test fixtures: SPEC-402, SPEC-400 ("safe to delete"), SPEC-397 (Bug310 MCP roundtrip fixture)
