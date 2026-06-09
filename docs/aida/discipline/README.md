# Discipline for AIDA-using sessions

How to work effectively with AIDA — habits, vocabulary, and workflow patterns for any project that uses AIDA (not about AIDA's internals). Scaffolded by `aida init`; edit them to fit your team — `aida init --refresh` won't overwrite your edits.

## The guides

| Guide | What it covers |
|-------|----------------|
| [`advisor-role.md`](advisor-role.md) | The advisor seat — its responsibilities, what it does *not* do, and the three autonomy modes |
| [`docs-lane.md`](docs-lane.md) | The single-writer docs lane (SPIKE-10 MVP) — one agent owns `docs/`, every other agent FLAGS via the `needs-docs` routing primitive instead of editing; drain via `/aida-burndown` filtered to docs + a periodic `/aida-docs-review` sweep; single-writer stays conventional |
| [`lifecycle-vocabulary.md`](lifecycle-vocabulary.md) | Precise words for each lifecycle state — committed vs pushed vs merged vs completed vs released |
| [`machinery-glossary.md`](machinery-glossary.md) | One-paragraph definitions of AIDA's orchestration / session / autonomy machinery — orchestrator, phase, drain, lease, role, scope, session, worktree, sentinel, batch, autonomy mode |
| [`tag-conventions.md`](tag-conventions.md) | The `aida:<subcommand>` colon-namespaced tag convention, plus the flat behavior/provenance namespace and existing colon namespaces (`batch:`, `lifecycle:`, …) |
| [`workflow-patterns.md`](workflow-patterns.md) | `/goal` prompt phrasing, and parallel-choice vs sequential-step UI |
| [`autonomous-burndown.md`](autonomous-burndown.md) | Hands-off backlog draining — worktree-isolated fan-out + integrator loop, pickability gate, punt-and-continue, `/aida-burndown` vs the orchestrator drain |
| [`session-discipline.md`](session-discipline.md) | Per-session habits — verify before filing, pause for design input, trust the reviewer, and more |
| [`skill-prompt-kinds.md`](skill-prompt-kinds.md) | Classifying `AskUserQuestion` prompts into mechanical vs design-fork kind, and their `--zen` pause behavior |
| [`substrate-as-bouncer.md`](substrate-as-bouncer.md) | The substrate-as-bouncer principle, detailing the pre-commit gitignored check hook and reviewer PR gates |
| [`agent-agnostic-vs-claude-specific.md`](agent-agnostic-vs-claude-specific.md) | Which discipline is universal (enforced by substrate gates, every agent) vs Claude Code-shaped convenience (`.claude/skills`, slash commands, memory pack) — so Codex/Antigravity users can tell the load-bearing from the optional |
| [`robust-project-root-resolution.md`](robust-project-root-resolution.md) | Project-root resolution fallbacks, explaining how skill-rendering gracefully handles missing git repositories |
| [`test-isolation.md`](test-isolation.md) | Parallel-test isolation under `cargo test` — the `EnvVarGuard` helper for process-global env mutation, the per-test temp-path pattern for subprocess plumbing |

**Companion:** `aida init --with-memories` writes the same discipline as persistent *memory* files (one fact per file), so the habits surface in-session — not only when these docs are read.
