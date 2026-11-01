# Product glossary

<!-- trace:STORY-2219 | ai:claude -->

Plain-language definitions of the words AIDA uses in its guides, commands and
messages. Each entry says what you see or do, and links to the page that
explains it in depth where one exists. Developer-facing machinery terms
(orchestrator, drain, lease, worktree and others) are defined in the
[machinery glossary](machinery-glossary.md), and lifecycle states (committed,
pushed, merged, completed, released) in the
[lifecycle vocabulary](lifecycle-vocabulary.md).

## Terms

### Agreed ID

The short ID a spec keeps for good: its type and a number, such as
`TASK-<number>`. It is unique across every clone of the project, because each
number is handed out once from a shared range. See [Support for air-gapped development](../../../../docs/air-gapped-development.md).

### Block

A small range of spec numbers set aside for one clone. The clone gives new
specs short agreed IDs from its own blocks, so it never has to ask anyone
else first. Blocks are carved from a [pool](#pool). See
[Support for air-gapped development](../../../../docs/air-gapped-development.md#the-parts).

### Brief

A short note that assigns a piece of work to a named agent, such as "pick up
this spec next". The agent reads it, does the work, and acknowledges it so it
stops showing up. A brief goes one way; for a conversation, use the
[mailbox](#mailbox). See [`aida brief`](../../../../docs/cli/06-roles-sessions.md#aida-brief).

### Child grant

A [grant](#grant) that a launcher gives an agent it starts, for one named
seat. Your session can hand out only the seats you listed when you entered
your own seat, and a child grant cannot be passed on again. See
[Entering seats and delegating child seats](../../../../docs/cli/06-roles-sessions.md#entering-seats-and-delegating-child-seats).

### Commit trailer

The spec ID in parentheses at the end of a commit subject, as in
`fix(api): handle missing ids (SPEC-ID)`. When that commit lands on the main
branch, AIDA marks the named spec completed. See the
[commit trailer convention](../../../../docs/commit-trailer-convention.md).

### Control plane

The part of AIDA that decides whether work may move forward and reports what
is in flight: the queue, seats, leases, reviews, merge holds and the status
views. The spec records themselves are the data it acts on, not part of it.

### Deputy

A helper you start in your own terminal with `aida deputy`. Agents hand it
commands to run under your seat grant, and it runs only what you allowed,
records each job and tells the submitting agent the result. See
[`aida deputy`](../../../../docs/cli/06-roles-sessions.md#aida-deputy).

### Grant

The authority your terminal session holds to act in a [seat](#seat). You get
one by running `aida role enter <seat>` at an interactive terminal; it expires
after a set time (24 hours by default) and works only for processes started
from that terminal. See
[Entering seats and delegating child seats](../../../../docs/cli/06-roles-sessions.md#entering-seats-and-delegating-child-seats).

### Handoff

A note a session writes before it ends so the next session can carry on
without the conversation that came before: what was done, what is open and
what to do next. AIDA keeps handoff notes in the project's `.aida/` folder.

### Hub

A Git server that the requirement store syncs through, such as GitHub, GitLab
or a repository on an internal host. A mirror that only copies another hub
counts as the same hub. See
[Support for air-gapped development](../../../../docs/air-gapped-development.md#the-parts).

### Local ID

A longer spec ID that also names the clone that made it, used when that clone
has no [block](#block) to draw short numbers from. Local IDs never collide, so
they need no coordination, and a spec can later be given an
[agreed ID](#agreed-id). You can give the clone a friendly label with
`aida node alias`. See
[Support for air-gapped development](../../../../docs/air-gapped-development.md#the-parts).

### Mailbox

AIDA's message channel between agents and people, for things that are not a
change of state: a heads-up, a question, a warning about a shared file.
Messages are kept, so a different agent can read them the next day. See
[the inter-agent mailbox](../../../../docs/mailbox.md).

### Mandate

Time-limited permission, started by a person at a terminal with
`aida mandate start`, for the advisor seat to do named things while that
person is away, such as shipping held pull requests. It lasts 8 hours by
default and at most 24, and never covers [operator-only](#operator-only)
specs. See [`aida mandate`](../../../../docs/cli/04-git-lifecycle.md#aida-mandate).

### Merge hold

A stop placed on a pull request so it cannot merge, even when its checks and
review pass. It shows as the `aida:merge-hold` label and a failing required
check. It stays until a person clears it with `aida merge-hold clear`, or
AIDA releases it in the narrow cases it allows, such as under a
[mandate](#mandate). See
[What is AIDA](../../../../docs/what-is-aida.md).

### Operator-only

A tag that keeps a spec in human hands. No [mandate](#mandate) covers it, so
its pull request waits for a person to merge, and the tag's effect remains even
if the tag is later removed. See
[`aida mandate`](../../../../docs/cli/04-git-lifecycle.md#aida-mandate).

### Pool

A range of one spec type's numbers given to one hub. Clones that sync through
that hub take their [blocks](#block) from it, so two hubs never hand out the
same number. See
[Support for air-gapped development](../../../../docs/air-gapped-development.md#the-parts).

### Ref

Git's name for a pointer to a commit, such as a branch. AIDA keeps its records
on the `aida-store` branch, called the store ref; a push to it succeeds only
if nobody else moved it first. See
[Support for air-gapped development](../../../../docs/air-gapped-development.md#the-parts).

### Review story

The story AIDA files when it opens a pull request, titled
`Review PR-<number>: …` and routed to the reviewer seat. Whoever picks it up
reviews that pull request and records a verdict. See
[the review process](../../../../docs/review-process.md).

### Rework round

One trip back to the implementer after a reviewer asks for changes.
`aida rework` returns the spec to active work with the reviewer's reasons, and
the implementer pushes fixes to the same pull request for another review. See
[`aida rework`](../../../../docs/cli/04-git-lifecycle.md#aida-rework).

### Role

A named job in AIDA's workflow, such as advisor, implementer, reviewer or
integrator. Work can be routed to a role, and the role decides what a session
wearing it is expected and allowed to do. See
[Roles and sessions](../../../../docs/cli/06-roles-sessions.md).

### Root

The one clone of a project that owns every spec number not yet handed out.
You choose it once with `aida node enroll-root`; it splits the numbers into
[pools](#pool). See
[Support for air-gapped development](../../../../docs/air-gapped-development.md#the-parts).

### Roster

The list of seats each person is allowed to take, kept per project. It is a
ceiling: `aida role enter` refuses a seat that is not on your roster, and
`aida team allow-seat` adds one. See
[Entering seats and delegating child seats](../../../../docs/cli/06-roles-sessions.md#entering-seats-and-delegating-child-seats).

### Seat

A [role](#role) as a place in the workflow that a session occupies, such as
the advisor seat. A seat can receive work and hold sessions, and some seats
allow only one active occupant at a time. See
[Roles and sessions](../../../../docs/cli/06-roles-sessions.md).

### Supervised spec

A sensitive spec, such as one touching security, containment, review or merge
authority, or one marked guided or keystone. A [mandate](#mandate) leaves its
[merge hold](#merge-hold) for a person unless that person started the
mandate with `--include-supervised`. See
[`aida mandate`](../../../../docs/cli/04-git-lifecycle.md#aida-mandate).

## How to add a term

This file is the canonical source for AIDA's product vocabulary. It is the
template master `aida-core/templates/.aida/discipline/product-glossary.md`.
Two outputs are generated from it: the CLI glossary (`aida docs glossary`,
published as the book's Glossary chapter by `make book-glossary`) and the
website glossary page (`python3 docs/showcase/build.py`). The terms that
earlier lived as `type=term` specs (local ID, role, seat, control plane) have
been folded in here; add new product vocabulary here, not as a term spec.

Each entry follows the same rules. (The website renderer reads one list item per line.)

- **Heading.** Add a `###` heading under Terms, in alphabetical order. The heading becomes the anchor: `### Merge hold` is `glossary.html#merge-hold`. Do not rename a heading once pages link to it.
- **Wording.** Write 1 to 3 plain sentences about what the reader sees or does, not how it is built. Do not cite spec IDs.
- **Links.** Link the page that explains the term in depth, relative to this file (`../../../../docs/<page>.md`), and other terms as `#anchor`.
- **Outputs.** Copy the master to `.aida/discipline/` (`make sync-templates`), rebuild the binary, run `make book-glossary` and `python3 docs/showcase/build.py`, and commit the regenerated files.
