# Discipline for AIDA-using sessions

How to work effectively with AIDA — habits, vocabulary, and workflow patterns for any project that uses AIDA (not about AIDA's internals). Scaffolded by `aida init`; edit them to fit your team — `aida init --refresh` won't overwrite your edits.

## The guides

| Guide | What it covers |
|-------|----------------|
| [`advisor-role.md`](advisor-role.md) | The advisor seat — its responsibilities, what it does *not* do, and the three autonomy modes |
| [`implementer-discipline.md`](implementer-discipline.md) | The implementer's six rules: one-spec-per-session, exit-after-ship, poll-briefs, ship-full-acceptance, read-pending-brief-banner, advise-escape — each linked to the runtime substrate-bouncer that enforces it |
| [`integrator-role.md`](integrator-role.md) | The integrator seat — owns the merge cascade (rebase, mechanical-conflict resolution, CI watch, squash-merge, `aida pull`); escalates semantic conflicts to the advisor, missing verdicts to the reviewer, real failures to the implementer |
| [`docs-lane.md`](docs-lane.md) | The single-writer docs lane (SPIKE-10 MVP) — one agent owns `docs/`, every other agent FLAGS via the `needs-docs` routing primitive instead of editing; drain via `/aida-burndown` filtered to docs + a periodic `/aida-docs-review` sweep; single-writer stays conventional |
| [`observation-discipline.md`](observation-discipline.md) | When to file an `aida findings add` observation vs an immediate BUG/TASK; the recurrence-as-promotion signal |
| [`lifecycle-vocabulary.md`](lifecycle-vocabulary.md) | Precise words for each lifecycle state — committed vs pushed vs merged vs completed vs released |
| [`machinery-glossary.md`](machinery-glossary.md) | One-paragraph definitions of AIDA's orchestration / session / autonomy machinery — orchestrator, phase, drain, lease, role, scope, session, worktree, sentinel, batch, autonomy mode |
| [`tag-conventions.md`](tag-conventions.md) | The `aida:<subcommand>` colon-namespaced tag convention, plus the flat behavior/provenance namespace and existing colon namespaces (`batch:`, `lifecycle:`, …) |
| [`workflow-patterns.md`](workflow-patterns.md) | `/goal` prompt phrasing, parallel-choice vs sequential-step UI, planning-pass file hygiene, and why recursive-failure-risk fixes ship at the keyboard not the drain |
| [`backlog-grooming.md`](backlog-grooming.md) | Grooming the Approved/Planned backlog — bucketing by tag/type/priority, what to queue vs archive vs escalate, and the file-overlap conflict heuristic |
| [`autonomous-burndown.md`](autonomous-burndown.md) | Draining a ready backlog hands-off — the worktree-isolated implementer fan-out + integrator loop, the pickability gate, punt-and-continue, never-down-tools, and `/aida-burndown` vs the orchestrator drain |
| [`git-sync-and-review.md`](git-sync-and-review.md) | Recovering when `aida pull` refuses (divergent branches) + the `aida review prompt` workflow — read-on-demand detail kept out of the always-in-context `AIDA.md` |
| [`session-discipline.md`](session-discipline.md) | Per-session habits — verify before filing, pause for design input, trust the reviewer, and more |
| [`skill-prompt-kinds.md`](skill-prompt-kinds.md) | Classifying `AskUserQuestion` prompts into mechanical vs design-fork kind, and their `--zen` pause behavior |
| [`substrate-as-bouncer.md`](substrate-as-bouncer.md) | The substrate-as-bouncer principle, detailing the pre-commit gitignored check hook and reviewer PR gates |
| [`agent-agnostic-vs-claude-specific.md`](agent-agnostic-vs-claude-specific.md) | Which discipline is universal (enforced by substrate gates, every agent) vs Claude Code-shaped convenience (`.claude/skills`, slash commands, memory pack) — so Codex/Antigravity users can tell the load-bearing from the optional |
| [`brief-polling.md`](brief-polling.md) | How agents should poll AIDA's brief surface — the scratchpad-drift failure mode and the `aida queue done` pending-brief banner |
| [`robust-project-root-resolution.md`](robust-project-root-resolution.md) | Project-root resolution fallbacks, explaining how skill-rendering gracefully handles missing git repositories |
| [`skill-cli-symmetry.md`](skill-cli-symmetry.md) | When a skill's deterministic slice ships as a CLI verb, the parent skill must *call* that verb in the same PR rather than re-implement the logic — the same anti-drift discipline as the CLI↔MCP mirror (STORY-82) |

**Companion:** `aida init --with-memories` writes the same discipline as persistent *memory* files (one fact per file), so the habits surface in-session — not only when these docs are read.
