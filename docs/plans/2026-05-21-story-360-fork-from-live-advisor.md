# STORY-360 — Implement fork-from-live advisor with cold-boot fallback

**Date:** 2026-05-21
**Specs:** STORY-360 (implements SPIKE-11 outcome)
**Status:** Implemented
**Complexity:** Medium

## Approach

Augment the `--no-human=both` orchestrator's `run_advisor` path with a fork-from-live preflight. When a live advisor session is registered (`aida advisor register` writes `~/.aida/advisor.toml`), the orchestrator copies the live session's JSONL transcript into the spec's worktree project slug under a freshly-minted UUID and runs `claude --resume <fork>` with the punt prompt — so the headless advisor boots with full in-flight context instead of a fresh substrate-only load. Discovery is graceful: no registration / dead session / oversized transcript / `fork_mode = "never"` all fall through to today's cold-boot path.

```
                    ┌──────────────────────────────────────┐
                    │   aida advisor register              │
                    │   → ~/.aida/advisor.toml             │
                    └────────────────┬─────────────────────┘
                                     │
   implementer punts ─► orchestrator ▼
                       │  discover_live_advisor_session()
                       │      │
                       │      ├── reg + alive + ≤cap → plan_fork()
                       │      │                          │
                       │      │     execute_fork()       │
                       │      │     (cp JSONL → spec/    │
                       │      │      slug/<fork>.jsonl)  │
                       │      │                          ▼
                       │      │     claude --resume <fork> -p /aida-advise
                       │      │
                       │      └── none / dead / oversized
                       │              ▼
                       │     cold-boot: claude --session-id <new> -p /aida-advise
                       ▼
                  AdvisorOutcome → orchestrator decides resume / escalate
```

## Decisions

- **Live registration over auto-discovery.** Two alternatives both lose: an mtime-only heuristic mis-fires because *every* Claude session on the project updates the same project-slug directory; an `AIDA_ADVISOR_SESSION_UUID` env var doesn't survive new terminals. Registration to `~/.aida/advisor.toml` is explicit, survives shell restarts, and is what `aida advisor status` reads. The env var stays as a complement; the mtime fallback exists but defaults *off*.
- **Fork JSONL lives under the spec's worktree project slug, not the source's.** SPIKE-11 verified cross-project-slug invariance: `claude --resume` works regardless of which slug the fork lives in. Writing under the spec's slug means a debugger reading transcripts back from a worktree (`aida headless tail` from inside the spec) sees the fork in the same place as every other headless-log it has.
- **`fork_mode` default `auto`, not `always`.** Auto means: fork when registered, cold-boot otherwise. The user opts into the cost (~$4 first fork) by running `aida advisor register`. Defaulting to `always` would surprise users with a cache-creation tax they didn't sign up for.
- **`keep_fork_jsonls = true` by default.** Audit trail. The fork JSONL is a record of exactly what the advisor saw at decision time; deleting it loses that.
- **Liveness = recent JSONL mtime OR live recorded PID.** Either signal alone is enough — claude can spend longer than the mtime window inside a single tool call, but the PID never lies. `process_probe::pid_is_alive` is already cross-platform.
- **No re-architecture of cold-boot.** The fork path is *additive*: it picks a different `claude` argv (`--resume <fork>` vs `--session-id <new>`) and an optional JSONL copy, then re-enters the existing cold-boot path. The downstream — response parsing, ledger append, comment-on-spec, escalation handshake — is identical.

## Files (in build order)

- **`aida-cli/src/advisor.rs`** (new) — `AdvisorConfig`, `AdvisorRegistration`, `discover_live_advisor_session`, `plan_fork`, `execute_fork`, `estimated_fork_cost_usd`, freshness window, discovery cascade. Self-contained module with its own test suite.
- **`aida-cli/src/main.rs`** — module declaration, `Command::Advisor` early dispatch (before storage init), `handle_advisor_command` / `handle_advisor_register` / `handle_advisor_status` / `locate_for_status`, and the `run_advisor` augmentation that calls `plan_fork` + `execute_fork` and chooses between `claude_headless_args` / `claude_headless_resume_args`.
- **`aida-cli/src/cli.rs`** — `AdvisorCommand` enum (Register / Unregister / Status), `Command::Advisor(AdvisorCommand)` variant.
- **`docs/autonomous-drain.md`** — new "Fork-from-live" subsection under the existing advisor-tier section: registration flow, cost trade-off, config keys, discovery cascade, JSONL destination.
- **`README.md`** — one-sentence pointer in the "Needs Attention" paragraph to fork-from-live + the docs link.

## Critical Files

- `aida-cli/src/advisor.rs` — `plan_fork`, `discover_live_advisor_session`, `is_alive`.
- `aida-cli/src/main.rs::run_advisor` (line ~53509 onward) — the augmentation site.
- `aida-cli/src/session.rs::claude_headless_resume_args` (line 851) — the SPIKE-7-compliant `--resume` argv builder reused by the fork path.
- `aida-cli/src/process_probe.rs::pid_is_alive` (line 230), `encode_cwd_for_projects` (line 188) — re-used for liveness + slug encoding.

## Reusable helpers

- `session::claude_project_dir(cwd)` — encodes a cwd into the `~/.claude/projects/<slug>/` form. Used to locate the fork's destination directory.
- `session::claude_headless_resume_args(prompt, sid)` — SPIKE-7-compliant `--resume` argv. Returned verbatim by `claude_args` when forking.
- `process_probe::pid_is_alive(pid)` — sysinfo-backed cross-platform PID liveness check, already used by orchestrator-context corroboration (BUG-233).
- `process_probe::probe_live_claude_sessions()` — used by `aida advisor register` to record the live `claude` PID at registration time.

## Risks + gotchas

- **Cost surprise** — first-fork cache-creation tax is real ($4 at 1.3 MB). Mitigated by `aida advisor status` showing the estimated $/fork at current transcript size, and the `max_source_size_mb` soft ceiling.
- **Stale registration** — `~/.aida/advisor.toml` could point to a session that has since closed. Mitigated by the liveness check (PID alive OR JSONL freshly written within the window) — discovery falls through to cold-boot when both signals are absent.
- **Fork JSONL pollution** — every punt produces a new JSONL under the spec's worktree project slug. `keep_fork_jsonls = false` opts into immediate cleanup; otherwise the fork JSONL stays for audit and is pruned by the existing `aida session prune` mechanism.
- **Tool-set inheritance** — the fork inherits the source advisor's toolset (the `--allowed-tools` from `/aida-advise` frontmatter still applies via the resume). If the source had ad-hoc tool changes mid-session, the fork sees them. Acceptable per SPIKE-11; tracked as a follow-up SPIKE.
- **Concurrent punts forking the same source** — two punts arriving within seconds each fork the same source JSONL. Both forks see identical state; no contention because the source is read-only from the fork's perspective.

## Tests (in `aida-cli/src/advisor.rs::tests`)

- `config_defaults_when_no_section` — missing `[advisor]` section → all defaults.
- `config_reads_all_advisor_keys` — `fork_mode`, `allow_mtime_fallback`, `keep_fork_jsonls`, `max_source_size_mb`.
- `config_ignores_other_sections` — `[hints]` etc. don't bleed into advisor config.
- `config_loads_from_disk` — real `.aida/config.toml` file path.
- `fork_mode_parse_accepts_known_values` — auto / always / never (case-insensitive); garbage → None.
- `never_mode_short_circuits_discovery` — `fork_mode = "never"` skips the discovery cascade entirely.
- `discovery_returns_none_when_unregistered` — no registration / env / fallback → None.
- `discovery_picks_registered_session_when_alive` — registered + JSONL fresh → Some, discovery = Registration.
- `discovery_falls_through_when_registered_jsonl_missing` — registration points at non-existent JSONL → None.
- `discovery_treats_stale_jsonl_as_dead` — JSONL mtime > ALIVE_JSONL_WINDOW and no PID → None.
- `plan_fork_destination_uses_spec_worktree_slug_not_source_slug` — explicit guarantee that the fork JSONL lands under the spec's slug.
- `plan_fork_respects_size_ceiling` — JSONL above `max_source_size_mb` → None.
- `registration_roundtrip` — `write_registration` / `read_registration` / `clear_registration` idempotent.
- `estimated_cost_scales_with_size` — $/fork roughly linear in MB.

## Verification (executable)

```bash
cargo build --bin aida
cargo test --bin aida advisor::

# CLI smoke
./target/debug/aida advisor --help
./target/debug/aida advisor status         # → "No live advisor registered."
CLAUDE_CODE_SESSION_ID=test-xxx ./target/debug/aida advisor register
./target/debug/aida advisor status         # → registered, source jsonl missing, cold-boot fallback
./target/debug/aida advisor unregister
```

## Followups

- **Calibration ledger (STORY-347)** is now unblocked — cold-boot and fork can run in parallel for the same punt and record both verdicts. The acceptance criteria mentioned in SPIKE-11 §"Composes with" is now satisfiable.
- **Toolset-of-fork investigation** — flagged as a follow-up SPIKE in the SPIKE-11 writeup. The fork inherits the source's tools; whether the advisor should run with a restricted toolset (read-only, no Bash) is undecided.
- **`aida session prune --forks`** — the SPIKE-11 writeup mentions this as a cleanup helper for accumulated fork JSONLs. Defer until the JSONLs actually pile up (`keep_fork_jsonls = false` covers the immediate case).
- **`aida advisor status --estimate <MB>`** — let a user model "what if I had a 5 MB transcript?" without registering. Pure UX nicety; not blocking.

## Related

- **SPIKE-11** — the research that motivates this story (`docs/spikes/2026-05-20-spike-11-session-forking.md`).
- **STORY-306** — the headless advisor escalation tier this story augments (cold-boot version stays intact).
- **STORY-347** — calibration ledger that now becomes implementable.
- Memory `feedback_headless_advisor_is_cold_boot` — updated to mark fork-from-live as implemented.
