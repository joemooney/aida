# Skill prompt kinds — classifying `AskUserQuestion` prompts

> Author guidance for AIDA skill templates. trace:STORY-287

When a skill template asks the user something, that prompt belongs to one
of two **kinds**. The kind decides whether the prompt pauses or
auto-resolves under the `--zen` autonomy mode. Classifying prompts well is
what makes `aida queue work --zen` ("advisor on standby") useful: the
mechanical clicks disappear, the real questions still reach the human.

## The two kinds

### `kind:confirmation`

A mechanical yes/no whose **default action is obvious**. The user almost
always says yes; the prompt exists as a courtesy pause, not because there
is a real decision. Examples:

- "Open this PR?"
- "All green — merge?"
- "Grab the next queued item?"
- "Mark the review story rejected?" (after the rejection is already decided)

Under `--zen`, a `kind:confirmation` prompt **auto-resolves to option 1**
(its first / recommended choice) without rendering interactive UI.

### `kind:design-fork`

A genuine choice between **meaningful alternatives**, where guessing wrong
has real cost. Reserved for choices the skill genuinely cannot make alone:
the spec is ambiguous, callers diverge, or the blast radius of a wrong
guess is large. Examples:

- "Scope this to `aida show` only, or unify status display across every
  CLI surface?"
- "The PR has commits with no `(REQ-ID)` trailer — how should the diff be
  attributed?"
- "Acceptance criteria are vague — tighten the spec, or accept the gap?"

Under `--zen`, a `kind:design-fork` prompt **always surfaces**. The advisor
is at the keyboard precisely to answer these.

### `kind:bug-spotted` (reserved)

A third kind — "the implementer found something mid-work; file it as a
BUG?" — is reserved for the `--no-human` punt slice (it routes through the
implementer-findings surface, STORY-285). It is not yet operative; until
that slice lands, classify a found-bug prompt as `design-fork` (pause-safe).

## How to classify

**Most prompts are `confirmation`.** `design-fork` should be *sparse and
meaningful* — if every prompt is a design-fork, `--zen` resolves nothing
and the mode is pointless. When in doubt, ask: *would the user ever
realistically answer anything but yes?* If no, it is a confirmation.

But err on the side of `design-fork` for the genuinely uncertain case:
auto-resolving a real question is worse than over-asking. That is also why
**an un-annotated prompt defaults to `design-fork`** — a missing annotation
fails safe (pauses) rather than wrongly auto-resolving.

## The option-1 convention

Auto-resolve picks **option 1** — so the *first* option a prompt lists must
be the **smallest-valuable-slice / lowest-risk default**. Subsequent
options expand scope or accept more risk. For a guard prompt ("ship a
half-done batch anyway?"), option 1 is the *safe refusal*, not the bypass.

This is the same discipline as `feedback_pushback_on_overengineering.md`:
ship the smallest correct thing, defer the rest as a follow-up.

## The annotation

Tag each prompt with an HTML comment **directly above** the prompt prose:

```markdown
<!-- kind:confirmation -->
Show the title and Summary. Ask explicitly: "Open this PR?"
```

HTML comments survive markdown rendering and are greppable for a future
lint (warn when an `AskUserQuestion`-style prompt carries no `kind:`).

## How the three autonomy modes consume the kind

| Mode | Persona | `kind:confirmation` | `kind:design-fork` |
|---|---|---|---|
| **Default** (no flag) | "Driving" — approves each step | Pause + ask | Pause + ask |
| **`--zen`** (`AIDA_ZEN=1`) | "Advisor on standby" | **Auto-resolve to option 1** | Pause + ask |
| **`--no-human`** (`AIDA_HEADLESS=1`) | "Absent" | Auto-resolve | *Punt* (future slice) |

`--no-human` > `--zen` > default. The headless drain is the stronger mode;
when both are set, `--no-human` wins. The `--no-human` punt of a design-fork
(pick a defensible default, file the deferred decision as a finding) is a
follow-up slice — it depends on the headless implementer (STORY-276) and the
findings-persistence surface (STORY-285). Until then `--zen` is the
operative mode and design-forks always pause.

## Where this is wired

- `aida queue work --zen` (and `AIDA_ZEN=1`) — `aida-cli`, sets the env var
  the launched session inherits.
- The four core skills carry kind annotations + an "Autonomy mode" section:
  `/aida-pickup`, `/aida-implement`, `/aida-pr`, `/aida-review`. Other
  skills are a follow-up.
- `docs/autonomous-drain.md` — the three-mode table + when to use each.

## Related

- STORY-287 — the three-mode autonomy taxonomy.
- `feedback_pause_for_design_input.md` — the existing discipline that the
  implementer should pause on design-laden choices; `--zen` keeps that for
  `design-fork`, drops it for `confirmation`.
- `feedback_pushback_on_overengineering.md` — the option-1 convention.
