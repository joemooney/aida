# Agent seat parity

This audit records whether each supported agent can occupy each AIDA authority
seat. “Works” means the command path is vendor-neutral and has been exercised;
“degraded” means the seat works but a vendor-native capability is unavailable.
The audit was refreshed for TASK-1279 on 2026-09-19.

## Matrix

| Seat / mode | Claude | Codex | Antigravity |
|---|---|---|---|
| Product / interactive | **Works** — `aida agent new claude --role product --show-context` | **Works** — `aida agent new codex --role product --show-context` | **Works** — `aida agent new antigravity --role product --show-context` |
| Product / headless | **Works** — `AIDA_SESSION_ROLE=product claude -p PROMPT` | **Works** — `AIDA_SESSION_ROLE=product codex exec PROMPT` | **Degraded** — scaffolded launcher; unattended dogfood is pending (TASK-1207) |
| Product / fork-from-live | **Works** through the Claude resume transport | **Degraded** — no Codex transcript-copy API; use the cold-boot scheduled tick (TASK-1308) | **Degraded** — no portable resume transport (TASK-1207) |
| Product / scheduled tick | **Works** — scheduler invokes `AIDA_SESSION_ROLE=product claude -p PROMPT` | **Works** — scheduler invokes `AIDA_SESSION_ROLE=product codex exec PROMPT` | **Degraded** — launcher scaffold only (TASK-1207) |
| Advisor / interactive | **Works** — `aida agent new claude --role advisor --show-context` | **Works** — `aida agent new codex --role advisor --show-context` | **Works** — `aida agent new antigravity --role advisor --show-context` |
| Advisor / headless | **Works** — `AIDA_HEADLESS_VENDOR=claude aida advisor watch --once` | **Works** — `AIDA_HEADLESS_VENDOR=codex aida advisor watch --once`; vendor-native cold boot | **Degraded** — cold boot exists; unattended dogfood pending (TASK-1207) |
| Advisor / fork-from-live | **Works** — `aida advisor register && aida advisor watch --once` | **Degraded** — watch falls back explicitly to a Codex cold boot; Codex has no portable transcript fork (TASK-1308) | **Degraded** — no portable transcript fork (TASK-1207) |
| Advisor / scheduled tick | **Works** — `aida advisor watch --once` | **Works** — `AIDA_HEADLESS_VENDOR=codex aida advisor watch --once` | **Degraded** — needs dogfood evidence (TASK-1207) |
| Implementer / interactive | **Works** — `aida agent new claude --role implementer --show-context` | **Works** — `aida agent new codex --role implementer --show-context` | **Works** — `aida agent new antigravity --role implementer --show-context` |
| Implementer / headless | **Works** — `aida queue work ID --no-human=both --vendor claude` | **Works** — `aida queue work ID --no-human=both --vendor codex` | **Degraded** — backend exists; drain dogfood pending (TASK-1207) |
| Implementer / fork-from-live | N/A — implementers are isolated cold boots | N/A — implementers are isolated cold boots | N/A — implementers are isolated cold boots |
| Implementer / scheduled tick | **Works** through queued drain/batch | **Works** through queued drain/batch with `--vendor codex` | **Degraded** — TASK-1207 |
| Reviewer / interactive | **Works** — `aida agent new claude --role reviewer --show-context` | **Works** — `aida agent new codex --role reviewer --show-context` | **Works** — `aida agent new antigravity --role reviewer --show-context` |
| Reviewer / headless | **Works** — reviewer phase of `aida queue work ID --no-human=both --vendor claude` | **Works** — reviewer phase of `aida queue work ID --no-human=both --vendor codex` | **Degraded** — TASK-1207 |
| Reviewer / fork-from-live | N/A — independent review must cold-boot | N/A — independent review must cold-boot | N/A — independent review must cold-boot |
| Reviewer / scheduled tick | **Works** through queued drain/batch | **Works** through queued drain/batch with `--vendor codex` | **Degraded** — TASK-1207 |
| Integrator / interactive | **Works** — `aida agent new claude --role integrator --show-context` | **Works** — `aida agent new codex --role integrator --show-context` | **Works** — `aida agent new antigravity --role integrator --show-context` |
| Integrator / headless | **Works** — `AIDA_SESSION_ROLE=integrator aida integrate --run --max 1` | **Works** — integration is CLI-native and vendor-neutral; same command | **Works** — integration is CLI-native and vendor-neutral; same command |
| Integrator / fork-from-live | N/A — integration consumes durable events, not a transcript | N/A — integration consumes durable events, not a transcript | N/A — integration consumes durable events, not a transcript |
| Integrator / scheduled tick | **Works** — `AIDA_SESSION_ROLE=integrator aida integrate --watch --max 1` | **Works** — vendor-neutral; same command | **Works** — vendor-neutral; same command |

## Shared parity guards

- `aida init --refresh` refreshes the generated AIDA block in `AGENTS.md`,
  giving Codex the durable discipline equivalent of Claude's memory pack while
  preserving all user-owned text outside the generated markers.
- `codex_hooks_match_settings` compares the embedded Claude and Codex hook
  event, matcher, and handler sets. Adding a supported handler to one wiring
  without the other fails the `aida-core` test suite in CI.
- Role-context snapshots are passed through `AIDA_AGENT_CONTEXT_FILE` for every
  launcher. Headless prompts describe the semantic action and explicitly ban
  conversational waiting; they do not require Codex to call Claude's
  `AskUserQuestion` or harness-only `Monitor` tools.

## Skill discovery (non-Claude skill packs)

AIDA scaffolds one portable skill pack at `.agents/skills/aida-*/SKILL.md` for
Codex and Antigravity. The pack is derived from the Claude skill set minus a
short, commented Claude-only list in `aida-core/templates/skill-inventory.toml`,
so a new skill reaches every vendor by default. The layout rests on this
matrix, run on 2026-09-25 against the local runtimes:

| | Codex 0.157.0 | Antigravity (agy) 1.2.11 |
|---|---|---|
| Discovers `.agents/skills/` | Yes (listed as a skill root) | Yes |
| Symlinked `SKILL.md` | **No**, silently skipped | Yes |
| Regular-file copy of `SKILL.md` | Yes | Yes |
| Hard-linked `SKILL.md` | Yes | Yes |
| Symlinked skill directory holding a regular `SKILL.md` | Yes | Not tested |
| Still discovers legacy `.codex/skills/` / `.antigravity/skills/` | Not tested | Not tested |
| Same `aida-*` name in `.claude/skills` and `.agents/skills` | Not tested | Not tested |

Consequences:

- `SKILL.md` under `.agents/skills/` must be a regular file. AIDA always writes
  one (with its checksum header after the YAML frontmatter); a symlink there
  is invisible to Codex.
- New installs never create `.codex/skills/` or `.antigravity/skills/`. An
  existing one that is a real directory keeps being maintained from the same
  derived list until the untested legacy cells above are run.
- AIDA manages only `aida-*` entries under `.agents/skills/`. Other tools'
  skills in that directory are never read, rewritten or pruned, and
  `aida scaffold refresh` does not adopt a `.agents/skills/` that holds no
  AIDA skill and no AIDA manifest.
- The pack is expected only when Codex or Antigravity is in the project's
  saved agent selection; a Claude-only project is not told it is missing.

An earlier note claimed `.codex/skills/` was the Codex discovery surface; the
matrix above supersedes it.

## Evidence captured on 2026-09-19

SCOPE OF THIS EVIDENCE, stated so the matrix is not read as more proven than it is:
four probes were run against roughly twenty-four non-N/A cells, and two of the four
are `--help` invocations. A `--help` proves a flag PARSES, not that the seat works.
So the cells below are supported by a mix of direct probes and reading; the
per-cell proving runs are TASK-1308's, and travel with the dogfood wave for the
same reason — that evidence accrues only from running one.

The following probes were run from the TASK-1279 worktree:

```text
$ aida queue next
TASK-1279 ... Routed for: implementer

$ aida agent new --help
accepted launchers include claude, codex, and antigravity; --role and
--show-context are available

$ aida advisor watch --help
--once, --dry-run, --triage-only, and bounded polling are available

$ cargo test -p aida-core codex_hooks_match_settings
test scaffolding::codex_hooks::tests::codex_hooks_match_settings ... ok
```

The Codex advisor implementation is additionally covered by the
`advisor_watch` unit tests and uses `spawn_vendor_headless_with_seat(...,
AgentSeat::Advisor, ...)`, so per-seat model/effort selection applies to the
cold-boot tick.

## Dogfood evidence

The repository's ordinary autonomous drains already establish Codex
implementer and reviewer parity. The coordination-seat acceptance requires two
real waves and cannot be replaced by a synthetic unit test:

1. Claude product + Codex advisor: run a batch wave, record started/completed/
   shelved/merged counts, and link the Codex advisor's merge-gate event.
2. Codex product + Claude advisor: repeat with the seats reversed.

Record both event-count blocks as comments on TASK-1308 using the SPIKE-82
count vocabulary. TASK-1308 is not complete until the first wave has merged at
least one drain-mode spec behind the Codex advisor gate.

The dogfood-wave proof was scoped out of TASK-1279 to TASK-1308 on 2026-09-20:
the evidence accrues only from running a wave, and no wave had run. The two
Codex **Degraded** cells above therefore cite TASK-1308, which is open, rather
than TASK-1279, which this document's own audit closes — a gap whose filed spec
is the spec that just closed cannot be found again by a later reader.
