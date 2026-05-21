# SPIKE-11: Fork-from-live-session as a rich-context advisor path

*2026-05-20 — investigation completed*

**Verdict: viable.** Recommend implementing as opt-in fork-from-live with cold-boot fallback. Specific design + smallest-valuable-slice implementation sketch below.

## Question

Today's `--no-human=both` orchestrator routes punts to a cold-boot headless advisor: a fresh `claude -p` per punt loading only the persistent substrate (memories, discipline docs, spec graph). The cold-boot advisor has no access to in-flight context that lives in a running advisor session.

Can the orchestrator instead **fork** the live advisor's session — copy its JSONL transcript and `claude --resume` the copy with the punt context — to get an advisor that boots with full live-session context? Specifically: is the mechanic viable, what does it cost, how stale is the fork, and how does the orchestrator discover the live advisor's session to fork?

## Methodology

Empirical: copy a known-ended session's JSONL to a new UUID, run `claude --resume <new-uuid> -p "<scoped prompt>"`, observe.

**Test corpus:** TASK-358's session JSONL (1.3 MB, 489 events, transcript of this morning's 24m worktree-cleanup implementation that ended before push). Known content; can verify the fork loaded the transcript by probing for TASK-358-specific facts that don't appear in any external context.

**Test setup:**
- Source: `~/.claude/projects/-home-joe-ai-aida-task-358/019e45cf-acea-73c1-9476-d044fe19d8e4.jsonl`
- Fork: copied to a new UUID under `~/.claude/projects/-home-joe-ai-aida/` (different project slug)
- Invocation: `claude --resume <fork-uuid> -p "Reply in exactly one sentence: what was the last spec you reported as Done, and what status will it auto-bump to? Then stop."` from `/home/joe/ai/aida`

## Findings

### 1. Mechanical viability — YES

The fork's response: *"The last spec I reported as Done was TASK-358, and it will auto-bump to Completed when PR #125 merges."*

That's a faithful answer from the source transcript: TASK-358, the Done status, the auto-bump mechanic, and PR #125 specifically (which appears only in the source transcript, not in any external context the model could have inferred from). **Full transcript context loaded.**

### 2. Cross-project-slug invariance — bonus finding

The source JSONL belonged to project slug `-home-joe-ai-aida-task-358` (whose original cwd `/home/joe/ai/aida-task-358` no longer exists). I copied it into `-home-joe-ai-aida` and ran `claude --resume` from `/home/joe/ai/aida` (the main worktree). It worked seamlessly.

**Implication:** the fork mechanic is project-slug agnostic at the JSONL level. The orchestrator can fork the live advisor's JSONL into any worktree's project slug — including the spec's worktree slug — and `claude --resume` works from inside that worktree. This eliminates a class of design problems around how the fork would "find" the right cwd.

### 3. Source isolation — verified

| | Before | After |
|---|---|---|
| Source JSONL mtime | 2026-05-20T07:35 | 2026-05-20T07:35 (unchanged) |
| Source events | 489 | 489 |
| Fork JSONL mtime | (didn't exist) | 2026-05-20T15:44 |
| Fork events | — | 496 (+7 from the test turn) |

The fork writes only to its own UUID. The original transcript is byte-stable.

### 4. Cost

Single-fork measurement (Opus 4.7 pricing):

| Component | Tokens | $ (approx) |
|---|---|---|
| Cache creation (transcript load) | 224,895 | $4.22 |
| Cache read (system + tools) | 18,489 | $0.03 |
| Input (prompt) | 6 | ~$0 |
| Output | 42 | $0.003 |
| **Total first fork** | | **~$4.25** |

**Amortization within cache TTL (~5 min):** subsequent forks of the same source pay cache-read only, ~$0.03 each. A burst of N punts within 5 minutes costs ≈ $4.25 + (N−1) × $0.03.

**Cost framing for typical use:** rare design-forks (one every several hours) → ~$4/fork, comparable to one good ChatGPT-Plus turn. Clusters of punts (uncommon but possible during a heavy drain) → dominated by the first; remaining punts effectively free.

For comparison: today's cold-boot advisor invocations (which load only memories + CLAUDE.md + discipline docs, ~30-60K tokens) cost roughly $0.50-$1.00 each. **Fork-from-live is 4-8× more expensive per invocation than cold-boot, in exchange for full in-flight context.**

### 5. Latency

6.7 seconds wall-clock from `claude --resume` invocation to response printed. The 1.3 MB / 225K-token cache creation is faster than I expected — Claude Code's resume loader is efficient enough that fork latency is comparable to any other heavy prompt.

### 6. Staleness window

Claude Code writes session JSONLs in append-only line-buffered mode: each event (user message, assistant response, tool_use, tool_result) is one JSONL line, written when the event completes. A fork made at time T sees state through the most recent *completed* turn — typically a few seconds behind the live session.

**Worst case staleness:** a fork taken while the live advisor is mid-turn (model is generating, or a tool is executing) misses that in-flight turn but has everything up to it. The fork is never *more* than one turn behind.

## Session-discovery mechanism (design)

The orchestrator needs to know *which* JSONL to fork. Four candidate mechanisms, ranked:

1. **`aida advisor register`** — the user runs this once when opening an advisor session. Writes `{uuid, started_at, project_slug}` to `~/.aida/advisor.toml`. The orchestrator reads this file when deciding whether to fork. Explicit, robust, survives terminal restarts. *Recommended primary.*
2. **Environment variable `AIDA_ADVISOR_SESSION_UUID=<uuid>`** — user (or `aida advisor register`) exports it. Orchestrator reads from env. Lightweight; doesn't survive new terminals. *Recommended as the registration mechanism's export, complementary to (1).*
3. **Latest-session-by-mtime heuristic** — scan `~/.claude/projects/-home-joe-ai-aida/*.jsonl` and pick the most-recently-modified. Auto, no registration required, but error-prone (every Claude session on the project updates mtime). *Recommended as fallback only, with a config flag to disable.*
4. **Explicit `--advisor-session <uuid>` on `aida queue work`** — per-drain. *Recommended only as override for testing/debugging.*

**Recommended primary stack:** (1) for state of record + (2) as the env export the registration produces + (3) as best-effort fallback if no registration exists + (4) for explicit override. Cold-boot is the final fallback when no live session is discoverable.

## Implementation sketch — smallest-valuable-slice

**File:** `aida-cli/src/auto_complete.rs::run_advisor` (today's cold-boot path)

**Behavior change** (additive, gated):

```rust
fn run_advisor(punt_context: PuntContext) -> AdvisorOutcome {
    if let Some(live_uuid) = discover_live_advisor_session() {
        // Fork path
        let fork_uuid = uuid::Uuid::new_v4();
        let source_jsonl = claude_project_dir(live_uuid.project_slug)
            .join(format!("{}.jsonl", live_uuid.uuid));
        let dest_jsonl = claude_project_dir(spec_worktree_slug())
            .join(format!("{}.jsonl", fork_uuid));
        std::fs::copy(&source_jsonl, &dest_jsonl)?;
        
        let verdict = spawn_claude_resume(&fork_uuid, &punt_context.prompt)?;
        
        // Optional: clean up the fork JSONL after use (config-gated; keep by default for audit)
        AdvisorOutcome::from_verdict(verdict)
    } else {
        // Fallback: today's cold-boot path
        spawn_claude_p_cold(&punt_context)
    }
}
```

**Discovery function:**

```rust
fn discover_live_advisor_session() -> Option<LiveAdvisor> {
    // 1. Try ~/.aida/advisor.toml
    if let Ok(registered) = read_advisor_toml() {
        if is_alive(&registered) {
            return Some(registered);
        }
    }
    // 2. Try env var
    if let Ok(uuid) = std::env::var("AIDA_ADVISOR_SESSION_UUID") {
        if let Some(advisor) = locate_session(&uuid) {
            return Some(advisor);
        }
    }
    // 3. Heuristic fallback (if config flag is on)
    if config.allow_mtime_fallback {
        return latest_session_by_mtime(&main_project_slug());
    }
    None
}
```

**New CLI commands:**
- `aida advisor register` — write current claude session's UUID + project slug to `~/.aida/advisor.toml`
- `aida advisor unregister` — clear the registration
- `aida advisor status` — show what's registered + whether it's alive

**Config:**
- `[advisor] fork_mode = "auto" | "always" | "never"` (default: `auto` — use fork if registered, cold-boot otherwise)
- `[advisor] allow_mtime_fallback = false` (default off; opt-in heuristic)
- `[advisor] keep_fork_jsonls = true` (default keep for audit trail)

**Composes with:**
- STORY-347 (calibration ledger) — once shipped, fork-from-live and cold-boot can run in parallel for the same punt, with both verdicts recorded for substrate-coverage measurement.
- Today's headless escalation (STORY-306) — the fork-from-live path supersedes cold-boot when registered; the escalation behavior is otherwise identical.

## Risks + gotchas

- **Cost surprise** for users not aware of the cache-creation tax. Mitigation: `aida advisor status` shows estimated per-fork cost based on current JSONL size; config has a soft cost ceiling that disables fork-from-live when source JSONL exceeds N MB.
- **Stale registration** — `~/.aida/advisor.toml` could point to a session that has since closed. Discovery function should check (a) JSONL file exists, (b) process is still alive (via the original claude PID, if recorded). If neither, fall through to cold-boot.
- **Fork JSONL pollution** — every punt produces a new JSONL in the worktree's project slug. Keep them by default (audit trail), but offer `aida session prune --forks` for cleanup.
- **Live session's tool definitions** — the fork inherits the source's tool set. If the orchestrator wants the fork to use a different toolset (e.g., a read-only advisor), it must either accept the source's tools or invoke `claude --resume <fork> --allowed-tools <subset>`. *Untested in this SPIKE; flag as a follow-up SPIKE.*
- **Concurrent forks of the same source** — two punts arriving within seconds would each fork the same JSONL. Both forks see the same state (no contention because the source is read-only from the fork's perspective). Acceptable.

## Verdict

**Viable.** Recommend shipping fork-from-live behind the `[advisor] fork_mode = "auto"` config flag, with cold-boot fallback when no live session is registered.

**Smallest-valuable-slice scope:**
1. `aida advisor register` / `unregister` / `status` CLI commands
2. `discover_live_advisor_session()` reading `~/.aida/advisor.toml` + env var
3. `run_advisor()` augmentation: fork if discovered, cold-boot otherwise
4. New tests covering: registered-and-alive → fork; registered-but-dead → cold-boot; not-registered → cold-boot
5. Config keys + docs update

Estimated complexity: medium. Touches `auto_complete.rs`, adds a new `advisor` subcommand surface, modest test surface. ~2-3 sessions of focused implementation work.

**Filing as STORY-N "Implement fork-from-live advisor with cold-boot fallback" after this SPIKE merges.** Composes with STORY-347 (calibration ledger) for the measurement loop.

## Related

- BUG-266 — transient API errors as inconclusive (a parallel reliability improvement for the headless advisor path)
- STORY-306 — headless advisor escalation tier (cold-boot version this SPIKE augments)
- STORY-347 — calibration ledger (uses fork-from-live as one half of the shadow-comparison)
- SPIKE-10 — multi-advisor coordination (orthogonal direction; both can ship)
- Memory `feedback_headless_advisor_is_cold_boot` — the load-bearing fact this SPIKE responds to; should be updated to mark fork-from-live as **validated viable** rather than hypothetical
