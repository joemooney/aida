# Contained seats get a private clone by default

## Summary

A contained agent seat now works in its own private clone of the repository
instead of a linked worktree of the main checkout. The clone has its own
`.git`, so the seat can commit, fetch and merge without write access to the
main checkout's git metadata, which stays read-only to it. Objects are borrowed
from the main checkout through git alternates, so a fresh clone copies almost
nothing.

This changes the default of `[contained] seat_tree` from `worktree` to `clone`.
The developer-only `AIDA_DEV_SEAT_CLONE` variable is removed.

## When a contained seat still gets a worktree

With `seat_tree` unset, a contained launch that a clone cannot serve keeps a
linked worktree, as before, and prints a one-line `note:` naming the reason:

- the checkout has no `origin` remote to clone from;
- cloning from `origin` fails (for example, offline access or credentials), or
  takes longer than 30 seconds;
- the session is a review session (`PR-N` / `MR-N`);
- the launch re-enters an existing `--path`;
- an explicit `--path` is not an `aida-seat-*` directory directly in the
  worktree parent.

If you set `seat_tree = "clone"` explicitly, these launches refuse instead and
name the reason. A missing origin says: add an origin remote or set
`seat_tree = "worktree"`.

<!-- trace:TASK-2300 | ai:codex -->
The origin-failure fallback removes the failed clone and its base pin before
creating a worktree from locally available refs. Its note names origin
reachability and credentials. An explicit `seat_tree = "clone"` still requires
a successful origin clone and refuses on failure. Failures preparing the
destination, resolving the base, pinning, checking out the branch or preparing
runtime state still refuse. A failed clone command may
also reflect a local clone-stage problem; the note asks for an origin check
without claiming to diagnose the cause.

## Opting out

To keep linked worktrees for contained seats, set this in `.aida/config.toml`:

```toml
[contained]
seat_tree = "worktree"
```

Uncontained sessions are unaffected and always use a worktree.

<!-- trace:TASK-2300 | ai:codex -->
An explicit `--path` using the reserved `aida-seat-*` clone location cannot
host a linked worktree. If origin cloning fails there, the launch refuses
before making a worktree and asks to drop `--path` for automatic placement
or choose an ordinary worktree path. It does not silently change that request.
