# User-facing text conventions

How AIDA writes the strings a *user* reads — CLI stdout/stderr, workflow
hints, banners, error messages, TUI chrome. The governing rule:

> **SPEC-ID citations belong in developer artifacts, never in user-facing
> output.**

A SPEC-ID (`TASK-85`, `STORY-249`, `EPIC-26`, …) is meaningful only to
someone who knows this project's requirement graph. To an AIDA developer
it's a useful breadcrumb back to the requirement that motivated a line of
code. To a first-user it's noise — an opaque token with no surrounding
context, meaningless without the project's history. trace:TASK-268

## The split

| SPEC-IDs **stay** in… | SPEC-IDs **leave**… |
|---|---|
| Commit messages (`(REQ-ID)` trailer) | Workflow hints (`ⓘ Workflow hint: …`) |
| Code comments — `//`, `///`, `/* */`, `# …`, `trace:` markers | Banners and section headers |
| Plan files under `docs/plans/` | Error messages and recovery hints |
| Spec descriptions and comments in the store | CLI stdout / stderr the user reads |
| Changelog / release notes | `aida <cmd> --help` text |
| Structured logs and telemetry (`~/.aida/usage.jsonl`) | TUI chrome — status strip, welcome panel, overlays |

The two columns are the same fact stated twice: developer artifacts are
*written for* people who hold the requirement graph in their head;
user-facing output is *not*.

## What "user-facing" excludes

A `// trace:TASK-85 | ai:claude` comment on a function is **not** user-facing
— it never reaches a terminal a user looks at. Leave it. Likewise the
`(REQ-ID)` in a commit message and the SPEC ids in a plan file: those
artifacts exist *for* developers.

The trap is a string that is *both* a code comment and user-facing output.
A `///` doc comment on a `clap` field is the prime example — `clap` renders
it into `aida <cmd> --help`. There, a SPEC-ID leaks to the user whether it's
a bare id, a `trace:` marker, or just mentioned in otherwise-normal prose
("...reuses the `STORY-122` usage log") — a user reading `--help` has no way
to tell a deliberate citation from an internal breadcrumb; either way it's
project-internal noise (TASK-1516). The fix is to keep the id as a plain `//`
comment above the item (still a developer artifact, still a code comment)
and reword the `///` doc comment so its prose doesn't need to name the id at
all. `aida-cli-lib/src/cli.rs`'s `doc_comment_is_provenance_leak` enforces
this — including the prose case — with a reviewed, per-line allowlist for the
rare help text that's deliberately developer-/operator-facing.

## Worked example — the `per TASK-85` wart

The hint that motivated this convention (TASK-268):

```
ⓘ Workflow hint: PR #47 has a reviewer story in the queue.
  Start the review with `aida queue work STORY-249` (or `aida queue work PR-47` per TASK-85).
```

`per TASK-85` cites the TASK that implemented `PR-N → review-story` routing.
A breadcrumb for AIDA developers; noise for everyone else. The fix
(TASK-267) recommends **one** command and drops the citation:

```
ⓘ Workflow hint: PR #47 ready for review.
  Start the review:  aida queue work PR-47
```

## If an alternative form is genuinely useful

Sometimes a second way to name the same thing helps the user (a PR number
*and* its spec id). Surface it by **what it is**, not by the SPEC that
implemented it:

- ✅ `(or by spec id:  aida queue work STORY-249)`
- ❌ `(or aida queue work PR-47 per TASK-85)`

`per <SPEC>` / `see <SPEC>` / `(trace:<SPEC>)` are developer phrasings — they
have no place in a string a user reads.

## Auditing

Grep user-facing string sites for SPEC-ID patterns:

```bash
# println/eprintln/format! output
rg -nE '(per|see) (TASK|BUG|STORY|EPIC|SPIKE|FR)-[0-9]' aida-cli/src aida-tui/src

# clap doc comments — any SPEC-ID at all (bare, trace:, or prose-embedded);
# same criterion `doc_comment_is_provenance_leak` and its unit tests enforce
rg -nE '^\s*///.*\b(STORY|TASK|BUG|EPIC|SPIKE|FR|CR|SPEC|ADR|PRIN)-[0-9]+' aida-cli-lib/src/cli.rs
```

Any hit inside a `println!` / `eprintln!` / `format!` that reaches a
terminal, or inside a `///` doc comment `clap` turns into `--help`, is a
wart — including a descriptive prose mention, not just a bare id or a
`trace:` marker (TASK-1516). Hits inside `//` comments, commit messages, and
plan files are fine.
