# Discipline for AIDA-using sessions

These docs describe how to work effectively with AIDA — the habits,
vocabulary, and workflow patterns that make an AIDA project run smoothly.
They are *not* about AIDA's own internals; they apply to **any** project
that uses AIDA to track requirements and drive AI coding sessions.

They were scaffolded into this project by `aida init`. They are yours now —
edit them to fit your team. `aida init --refresh` will not overwrite them
once you have made them your own.

## Why this exists

A project that adopts AIDA gets the tool immediately, but the *use pattern*
— how to route work, how to talk about lifecycle state, when to pause for
input — is learned the hard way, one papercut at a time. This pack ships
that pattern up front so a new project starts with the discipline already
in hand.

## The guides

| Guide | What it covers |
|-------|----------------|
| [`advisor-role.md`](advisor-role.md) | The advisor seat — its responsibilities, what it does *not* do, and the three autonomy modes |
| [`implementer-discipline.md`](implementer-discipline.md) | The implementer's six rules: one-spec-per-session, exit-after-ship, poll-briefs, ship-full-acceptance, read-pending-brief-banner, advise-escape — each linked to the runtime substrate-bouncer that enforces it |
| [`observation-discipline.md`](observation-discipline.md) | When to file an `aida findings add` observation vs an immediate BUG/TASK; the recurrence-as-promotion signal |
| [`lifecycle-vocabulary.md`](lifecycle-vocabulary.md) | Precise words for each lifecycle state — committed vs pushed vs merged vs completed vs released |
| [`machinery-glossary.md`](machinery-glossary.md) | One-paragraph definitions of AIDA's orchestration / session / autonomy machinery — orchestrator, phase, drain, lease, role, scope, session, worktree, sentinel, batch, autonomy mode |
| [`tag-conventions.md`](tag-conventions.md) | The `aida:<subcommand>` colon-namespaced tag convention, plus the flat behavior/provenance namespace and existing colon namespaces (`batch:`, `lifecycle:`, …) |
| [`workflow-patterns.md`](workflow-patterns.md) | `/goal` prompt phrasing, and parallel-choice vs sequential-step UI |
| [`session-discipline.md`](session-discipline.md) | Per-session habits — verify before filing, pause for design input, trust the reviewer, and more |
| [`skill-prompt-kinds.md`](skill-prompt-kinds.md) | Classifying `AskUserQuestion` prompts into mechanical vs design-fork kind, and their `--zen` pause behavior |
| [`substrate-as-bouncer.md`](substrate-as-bouncer.md) | The substrate-as-bouncer principle, detailing the pre-commit gitignored check hook and reviewer PR gates |
| [`brief-polling.md`](brief-polling.md) | How agents should poll AIDA's brief surface — the scratchpad-drift failure mode and the `aida queue done` pending-brief banner |
| [`robust-project-root-resolution.md`](robust-project-root-resolution.md) | Project-root resolution fallbacks, explaining how skill-rendering gracefully handles missing git repositories |

## The companion: the starter memory pack

`aida init --with-memories` writes the same discipline as a set of
persistent *memory* files (one fact per file) into the Claude Code project
memory directory, so the habits surface automatically during a session
rather than only when someone reads these docs. The memory pack and these
docs say the same things; the docs are the long form, the memories are the
in-session nudge.
