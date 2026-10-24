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
- the session is a review session (`PR-N` / `MR-N`);
- the launch re-enters an existing `--path`;
- an explicit `--path` is not an `aida-seat-*` directory directly in the
  worktree parent.

If you set `seat_tree = "clone"` explicitly, these launches refuse instead and
name the reason. A missing origin says: add an origin remote or set
`seat_tree = "worktree"`.

## Opting out

To keep linked worktrees for contained seats, set this in `.aida/config.toml`:

```toml
[contained]
seat_tree = "worktree"
```

Uncontained sessions are unaffected and always use a worktree.
