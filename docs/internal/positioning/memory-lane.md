# The Memory Lane

<!-- trace:TASK-1216 | ai:codex -->

The memory lane is AIDA's entry adoption lane: a quiet requirements store and
project notepad before any queue, drain, role, orchestrator, hook, or team
machinery is invited in.

Use it when you want the project to remember things across ordinary agent
chats, but you are not yet asking AIDA to run work. In this lane, AIDA is not a
process manager. It is the durable place an agent can query before answering
and update when the conversation creates a fact worth keeping.

## What It Is For

- Capturing decisions, requirements, reminders, and "why is this like this?"
  notes as stable specs instead of scrollback.
- Giving Claude, Codex, and other agents a shared CLI-readable record without
  making a user learn the autonomous workflow first.
- Letting a repo grow from "please remember this" to "coordinate this work"
  only when the extra machinery earns its keep.

## What Stays Out

The memory lane deliberately leaves out work execution surfaces:

- no queue or drain setup;
- no role roster;
- no orchestrator or lease model;
- no agent hooks or command packs required for daily use.

Those features are AIDA's team/adoption lane. They are useful once work needs
coordination, review, and lifecycle pressure. They should not be the price of
using AIDA as a project memory.

## The Quiet Footprint

A memory-lane project should be boring in `git status`.

The expected visible footprint is:

- `.aida/config.toml` — the tracked project pointer/config that tells AIDA where
  the store lives;
- `.aida/project.toml` — the tracked manifest that says what the project is and
  why it exists;
- `.gitignore` rules that deny `.aida/*` by default while allow-listing those
  tracked exceptions;
- the git-canonical store itself, kept on the `aida-store` orphan branch and in
  the local `.aida-store/` worktree.

Everything else under `.aida/` is runtime state: caches, sessions, logs, local
mailboxes, and other per-clone files. Those files are intentionally ignored.
The rule is simple: if a file under `.aida/` should travel with the repository,
it needs an explicit `!.aida/<file>` allow-line and a clear explanation in
review.

## How To Recognize It

When someone notices `.aida/` in a repository, the answer should be short:

> This project uses AIDA's memory lane. `.aida/config.toml` points to the
> requirement store, `.aida/project.toml` describes the project, and the rest of
> `.aida/` is local runtime state ignored by git.

That is the adoption promise: discoverable when inspected, invisible during
normal work.
