# Commit Trailer Convention

<!-- trace:TASK-1183 | ai:codex -->

The portable unit is the commit message convention, not AIDA. A team can
adopt this in any Git repository without installing AIDA, running a server, or
moving its planning system.

Use this subject shape for commits that implement tracked work:

```text
[AI:tool] type(scope): description (REQ-ID)
```

Examples:

```text
[AI:codex] feat(auth): add password reset tokens (STORY-42)
[AI:claude:med] fix(api): handle missing customer ids (BUG-17)
[AI:codex+claude] docs(onboarding): explain local setup (TASK-8)
feat(billing): add invoice export (STORY-44)
```

The pieces are intentionally small:

- `[AI:tool]` says an AI coding tool materially contributed to the commit.
  Use `[AI:tool1+tool2]` for mixed agent authorship and `:med` or `:low`
  when the AI contribution was moderate or small. Omit the prefix for
  human-only work.
- `type(scope): description` follows the Conventional Commits shape. The
  scope is optional.
- `(REQ-ID)` names the requirement, ticket, issue, story, bug, or task this
  commit implements. The ID does not need to come from AIDA. It can be a Jira
  key, GitHub issue number, Linear issue, local markdown spec id, or any other
  stable work id your team already uses.

## Copy-Paste Commit Hook

Save this as `.git/hooks/commit-msg` and run `chmod +x .git/hooks/commit-msg`.
It validates only the commit message. It does not call AIDA or any other
project tool.

```sh
#!/usr/bin/env sh
set -eu

msg_file="$1"
subject="$(sed -n '1p' "$msg_file")"

# Allow Git-generated messages that are not normal feature/fix/docs commits.
case "$subject" in
  Merge\ *|Revert\ \"*|fixup!\ *|squash!\ *) exit 0 ;;
esac

ai='(\[AI:[A-Za-z0-9_-]+(\+[A-Za-z0-9_-]+)*(:high|:med|:low)?\] )?'
type='(feat|fix|docs|style|refactor|perf|test|build|ci|chore|revert)'
scope='(\([A-Za-z0-9_,/ -]+\))?'
req='\(([A-Z][A-Z0-9_-]*-[0-9][A-Z0-9_-]*|#[0-9]+)\)'

if printf '%s\n' "$subject" | grep -Eq "^$ai$type$scope: .+"; then
  subject_without_ai="$(printf '%s\n' "$subject" | sed -E 's/^\[AI:[^]]+\] //')"
  case "$subject_without_ai" in
    feat*|fix*)
      if ! printf '%s\n' "$subject" | grep -Eq " $req$"; then
        echo "commit-msg: feat/fix commits must end with a work id like (STORY-42)" >&2
        exit 1
      fi
      ;;
  esac
  exit 0
fi

cat >&2 <<'EOF'
commit-msg: expected
  [AI:tool] type(scope): description (REQ-ID)

Examples:
  [AI:codex] feat(auth): add password reset tokens (STORY-42)
  [AI:claude:med] fix(api): handle missing customer ids (BUG-17)
  docs(readme): clarify local setup
EOF
exit 1
```

This hook treats the AI prefix as optional because the convention is useful
for human-only teams too. If your team wants every AI-assisted commit tagged,
make that a review rule: commits that contain AI-authored code should carry
the `[AI:tool]` prefix.

## What Becomes Measurable

Once the convention is followed consistently, plain Git history can answer
aggregate questions without reading source code or asking developers to fill
out timesheets:

- How many shipped commits were AI-assisted versus human-only?
- Which AI tools appear in the commit corpus?
- How often do AI-assisted commits land by type, such as `feat`, `fix`, or
  `test`?
- Which tracked work ids were implemented by AI-assisted commits?
- How does lead time or rework rate compare between AI-assisted and
  human-only buckets, if your team already has the timestamps and status data
  needed to compute those outcomes?

Those measurements are repository-level signals. They are useful for deciding
whether the team should invest more or less in agent workflows, where review
load is moving, and which kinds of work are good candidates for AI assistance.

## Aggregate-Only Constraint

Report these metrics only in aggregate. Do not publish per-developer AI-use
scorecards, rankings, or productivity claims from the trailer.

The reason is practical, not sentimental: the trailer is a coordination and
measurement signal. If people are scored on it, they will optimize the label
instead of the work. That destroys the dataset. Aggregate reporting keeps the
incentive aligned with the thing worth measuring: whether the team workflow is
improving.

Good reports:

- "42% of shipped commits this month carried an AI prefix."
- "AI-assisted commits were mostly docs, tests, and small fixes."
- "Review rework was lower for AI-assisted test changes and higher for
  AI-assisted API changes."

Bad reports:

- "Alice used AI on 70% of commits and Bob used it on 10%."
- "Rank engineers by AI-assisted commits shipped."
- "Use the trailer as an individual performance metric."

## Limitation: Self-Reported

The trailer is self-reported. It is a good-faith signal, not an audit trail.
It does not prove who typed code, how much of a diff came from an AI model, or
whether the model's output was accepted unchanged.

That limitation is acceptable when the data is aggregate-only and non-punitive.
It is not acceptable for compliance, compensation, surveillance, or individual
performance management. If you need those, this convention is the wrong tool.

## Where AIDA Fits

AIDA can consume this convention and combine it with specs, trace comments,
queues, reviews, and lifecycle status. None of that is required to start. The
first adoption step is just the commit subject convention plus the hook above.
