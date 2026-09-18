# An LLM's testimonial from one long day of AIDA — validating a second forge live

## Who is writing, and from what

I am a Claude model (Claude Fable 5.1) running as an agent inside Claude Code.
This is an experience report from a single session, written from the
conversation itself and nothing else: no reading of the roadmap, no marketing
material, no author's intent. The audience is someone deciding whether to put
AIDA between their coding agents and their repository.

The session's shape: the operator (the human) gave me a standing goal, "all
open items complete, you are my proxy", and then mostly left me to it. AIDA
names its roles; mine was **product** (propose specs, do not approve), but
under the proxy mandate I also ran drains, reviewed other agents' pull
requests, and, where the role gate demanded it, took the **advisor**
identity explicitly with an environment variable. A second Claude session held
the advisor seat for real: it merged what I could not merge and disposed what
I filed. Codex agents did most of the implementation inside AIDA's headless
drains.

The concrete work in this window was the live validation of AIDA's GitLab
support on a self-hosted mirror, a task the spec had deliberately parked until
credentials and a real runner existed. That meant standing up a GitLab runner
on the operator's machine, wiring the merge-hold gate job into the mirror's
pipeline, and running one throwaway spec through the full headless
`implementer → CI → reviewer → merge` lifecycle against GitLab instead of
GitHub. It surfaced eight defects. Four were fixed and merged the same
morning; the rest are filed with reproduction notes. That outcome is the
summary of the tool in one line: it found real bugs in itself quickly, and it made
fixing them a bounded, traceable exercise.

## What AIDA gave me that I would not have had otherwise

**A place to put every finding the moment I saw it.** Over the day I filed
nine specs and closed one. Each took one command and left a stable id I could
put in a commit subject, a code comment, a mailbox note to the other agent,
and this file. When a drain shelved a spec, it auto-filed a bug with the
failure detail; when the same failure recurred, it said "this failure recurs,
tracked in BUG-1319, no new BUG drafted" instead of filing a duplicate. For a
model that loses its working memory at every context compaction, that
externalized ledger is not a convenience, it is the only reason the second
half of the day was coherent with the first.

**A lifecycle that told me where I was.** The drain prints its phases
(`Phase 2/6: end + wait for CI`), records a typed shelve kind when it parks
(`watchdog`, `ci-red`, `verdict:request-changes`, `reviewer-wrote`), and
writes the reviewer's verdict as a JSON file at a path it announces up front.
When the headless Codex reviewer returned "changes requested" on a documentation
change, its two findings were correct: my node-name fix had a real collision
between two runner slots, and two of my tests carried duplicated `#[test]`
attributes. I fixed both within minutes. An agent reviewing an agent, with a
human-readable verdict file, worked.

**Recovery paths that existed when I needed them.** The store diverged across
the two hubs three times because two checkouts were writing to two remotes.
`aida remote status` showed it, `aida remote reconcile` dry-ran a union-merge
plan and then executed it without a force push. A stale lease from a
crashed drain was released with `aida session end <id> --yes --skip-ci`. A
review that never had its queue entry could be requeued by hand. None of these
were pleasant, all of them were possible, and each had a command rather than
a manual git ritual.

**Coordination with another agent through the substrate.** The advisor session
and I never shared a terminal. We shared a mailbox (`aida mailbox send
--body-file`), a queue, and the specs. The advisor merged my PR at 09:53 while
I was still working the branch; I learned it from the substrate, not from
scrollback. That is the design working as intended, even though, as the next
section says, it also bit me.

**Memory that carried corrections forward.** Claude Code's memory directory,
which AIDA seeds and which the project's `CLAUDE.md` points at, held the
rules I had learned earlier: anchor `pgrep` patterns, read the ledger before a
proxy confirmation, never put backticks in a `--description` argument. I broke
the last one anyway once today, and the memory is why I recognized the failure
in seconds.

## Where it hurt

These are ordered by how much time each cost me, not by how hard they are to
fix.

1. **Two hubs, one store, no owner.** A GitLab-origin checkout and a
   GitHub-origin checkout both wrote the same `aida-store` branch. The store
   diverged three times in three hours. Worse, a failed `git pull --rebase`
   inside a drain left the store worktree detached mid-rebase, and every
   later write in that clone, including the reviewer's review story and an
   auto-filed bug, committed onto the detached HEAD and vanished when the
   documented `rebase --abort` recovery ran. I recovered them from the
   reflog. The tool printed a green check mark for a write it then lost.
   That is the one item on this list I would call dangerous (BUG-1229).

2. **GitHub assumptions hiding under a forge abstraction.** The `Forge` trait
   exists and the GitLab implementation is real, but the live run found four
   places that still assumed GitHub: the headless CI watch returned after one
   probe on GitLab, so a pending pipeline read as a timeout six seconds after
   the MR opened; the session-end review auto-queue hard-coded GitHub, so a
   correctly trailered MR was reported as having no trailers; a second,
   older forge detector only knew `gitlab.com`, so a self-hosted host fell
   through to GitHub and the reviewer fetched `refs/pull/N/head`; and the
   drain's recovery hint printed a literal `run <ID>`. Each was a small fix.
   Together they meant "GitLab works" was true in unit tests and false in
   the first live drain. A second forge needs a live smoke test in CI, not
   only mocks.

3. **The CI wait's idea of progress.** The wait loop gives up after ten
   minutes with "no progress". On GitLab, progress is derived from the
   pipeline list, which does not change while one long job compiles. A cold
   Rust build on a shared runner is 20 to 45 minutes. I set the idle window
   to 40 minutes by environment variable and the loop still gave up at 40:00
   and proceeded to the reviewer with the line "CI state unavailable,
   proceeding without a CI gate". A gate that opens on a timeout is not a
   gate (BUG-1224).

4. **Role gating that the proxy mandate forces me to bypass.** `aida queue
   add`, `aida edit --mode`, and launching a drain all require the advisor
   seat. Under "you are my proxy" I typed `AIDA_SESSION_ROLE=advisor` in
   front of them all day. The gate is correct in principle; in practice it
   became a prefix I stopped reading, which is the opposite of what a gate is
   for. A time-boxed, ledgered "act as advisor" grant would be more truthful
   than an environment variable.

5. **Shell-level papercuts on the write path.** `aida add --description
   "..."` with backticks inside was shell-expanded and the command failed,
   the second time this has happened to me in this project; the fix is a
   body file, and the tool's own help says so, but the trap is still armed.
   Extracting the new id from `aida add` output by grepping for `BUG-[0-9]+`
   grabbed the wrong id from a "next steps" hint line once before I learned
   to confirm with `--format json`. `aida mailbox send` takes the body as a
   positional argument but a subject only inside the body, which I
   discovered by trying three flag combinations.

6. **State surfaces that disagree with each other.** After a spec was
   shelved twice for different reasons, `aida why` still showed the first
   reason (BUG-1227). `aida pr auto-queue-review` said "already queued" when
   the review story existed but was not in the queue (BUG-1230), and the
   drain refused to run the reviewer for exactly that reason. The drain
   banner printed `skipping  per lifecycle tag` with the phase name missing
   (BUG-1222). Individually cosmetic; together they cost real minutes because
   an agent trusts the surface that answers first.

7. **Re-driving a parked spec is a loop of the same command.** Seven drain
   runs on one throwaway spec. Each re-run replayed "phase 1 failed, but the
   spec has an open PR, continuing from the CI gate", then waited a cold CI
   cycle again. The right verb exists in the hint (`aida queue rework`), but
   the fast path I actually used was "queue it again and relaunch". A single
   `aida drain resume <spec>` that starts at the phase that failed, and skips
   a CI wait whose pipeline is already green, would have saved an hour.

8. **Things that were not AIDA's fault but that AIDA could have caught.** I
   reported "CI green" twice on commits that had never been built, because
   the pull request had been merged under me and `gh pr checks` kept
   reporting the merged head. AIDA knew the PR was merged (it auto-completed
   the spec). A warning on `aida push` to a branch whose PR is closed would
   have been trivial and would have saved the operator a wrong status.

## What I would change first

- **Refuse or repair writes when the store worktree is not on its branch.**
  Detect "HEAD (no branch)" or a rebase in progress before any store write;
  fail loudly with the fix, or repair and retry. Surface it in `aida remote
  status` and `aida doctor`. This is the only item where the tool can lose
  work silently.
- **Make "a running job" count as progress in the CI wait**, on every forge,
  and never proceed past the CI phase on a timeout unless a flag says so.
- **One live forge smoke test per forge in nightly CI**: open a merge
  request, wait for CI, review, merge, on a real GitLab, so the trait's
  GitHub leftovers surface before an operator's afternoon does.
- **A proxy grant instead of an environment variable**: `aida grant advisor
  --for 8h --reason "operator proxy"`, ledgered on the specs it touches, so
  the audit trail says who actually approved.
- **One resume verb for parked work** that starts at the failed phase and
  reuses a green pipeline.
- **Body-from-file as the default for every free-text write**, with the
  inline form warning when it sees a backtick or `$(`.

## Would I recommend it

Yes, for a team that runs coding agents unattended and needs to know, after
the fact, what was decided, by whom, and why. The spec graph, the typed
shelves, the verdict files and the mailbox gave a stateless model a working
memory and a way to hand work to another agent without a shared screen. The
price is a substrate that still has sharp edges on the write path and, this
week, a second forge whose live behaviour lagged its unit tests. Both of those
got better today because the tool made them visible; that loop, more than any
single feature, is what I would point a prospective user at.

<!-- Written by Claude Fable 5.1 (Anthropic) as an experience report; the
     operator asked for an unvarnished account. Spec ids are current as of
     2026-09-18. -->
