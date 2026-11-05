# AIDA git-mirror verb surface

A reference for the convention behind AIDA's git-mirror verbs — `aida fetch`,
`pull`, `push`, `rebase` — so new verbs stay consistent and contributors don't
re-derive the rules from scattered spec descriptions.

> Scope: this documents the **git-mirror** verbs (the ones that act across
> AIDA's two git legs). It is not a full CLI reference — run `aida <cmd> --help`
> for the authoritative flags. trace:TASK-109

## The two legs

Every AIDA project is two git histories that move together:

- **code leg** — your normal branch on `origin` (the working tree).
- **store leg** — the orphan `aida-store` branch on `origin` (the spec graph,
  one YAML per requirement), checked out at the gitignored `.aida-store/`
  worktree.

A spec change and the code that delivers it land in *different* legs. The
git-mirror verbs exist so you operate on both at once instead of remembering to
sync each by hand.

## The thesis: a verb earns its name by bundling both legs

An AIDA git verb is worth having only when it does something meaningful across
**both** legs. If it would just shell out to plain `git` on the code leg, it's
vestigial — use `git`. The verbs below all bundle; the deliberate non-mirrors
further down do not, and that's why they aren't AIDA verbs.

## The verb grid

| Verb | Bundles | Scopes | Dry-run | JSON | Notes |
|------|---------|--------|---------|------|-------|
| `aida fetch` | code + store refs | `--code-only` / `--store-only` | n/a (read-only) | — | Refreshes remote refs without merging or touching the worktree, so behind-counts / prechecks see current remote state. |
| `aida pull` | code + store | `--code-only` / `--store-only` | `--dry-run` | `--json` | Code leg is `--ff-only` by design; store leg is `--rebase`. See "Divergent-branch recovery" in `CLAUDE.md`. |
| `aida push` | code + store | `--code-only` / `--store-only` | `--dry-run` | `--json` | |
| `aida rebase` | classifies branch vs base across the workflow | — | `--dry-run` | `--json` | Stateless classifier — safe to invoke anywhere; reports state and suggests the next action. |

Adjacent surfaces that are *not* pure git-mirror verbs:

- **`aida session start` / `end`** — worktree-managed lifecycle (lease + branch
  + worktree create/remove), not a thin git mirror.
- **`aida db sync --pull --push`** — the older store-only sync surface, largely
  superseded by `aida pull` / `aida push`. Prefer the mirror verbs for new work.

## Intentional non-mirrors (use plain git)

These have no meaningful store-leg dimension, so AIDA deliberately does **not**
wrap them:

- **merge** — AIDA's workflow is rebase-first; there's no cross-leg merge use
  case.
- **stash** — git's stash works fine; nothing store-specific to add.
- **cherry-pick / tag** — code-leg operations with no store dimension.
- **log / diff / show** — AIDA reuses these names for *different* semantics
  (`aida show` prints SPEC details, not a git object), so it never shadows the
  git meaning.

## Conventions a new git-mirror verb must follow

1. **Bundle both legs by default.** The bare verb acts on code + store. Narrow
   with `--code-only` / `--store-only` — never invert the default.
2. **Name the two-leg behavior in the help opening line.** The first line of
   `--help` says it touches both legs (see `aida pull` / `aida push`), so the
   bundling is discoverable without reading the flag list.
3. **Surface what each leg will do before acting** where the action is
   non-trivial (the pre-action summary / dry-run output names both legs).
4. **Offer `--dry-run` for any acting verb.** It fetches the in-scope legs and
   prints what each *would* do (commit count + subjects), then exits 0 without
   acting. Honors `--code-only` / `--store-only`. (Read-only verbs like `fetch`
   don't need it.)
5. **Offer `--json` for machine consumers.** Same intent across verbs: emit the
   dry-run plan as JSON (`--json` implies `--dry-run` on `pull` / `push`).

## Related

- `STORY-115` — the git-mirror verb surface umbrella.
- `STORY-114` — `aida rebase`, the first published example of the convention.
- `CLAUDE.md` "Divergent-branch recovery" — the `--ff-only` (code) vs `--rebase`
  (store) split and the recovery recipe when the code leg refuses.
- Daily-commands sections of `CLAUDE.md` / scaffolded `AGENTS.md` — quick usage.
