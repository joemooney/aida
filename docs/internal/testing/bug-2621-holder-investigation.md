# BUG-2621 round 2: incident evidence unavailable

<!-- trace:BUG-2621 | ai:codex -->

Investigation on 2026-11-16 in the native round-2 seat did not establish the
actual admission-lock holder during the reported 3122-failure episode.
The binding advisor ruling requires that identification before implementation.
No holder identity or purpose is inferred from the error text.

Observed evidence:

- `aida show BUG-2621 --full --comments` contains the failure count and
  `blocked-retired-controllers` report, but no holder PID, start identity,
  descriptor inventory, or lock inode identifying that episode's holder.
- Neither `/home/dirtlab/ai/aida/.aida/review-watch` nor its `auto-train`
  counterpart exists in this seat's primary checkout view.
- `/proc/locks` returned no entries in this execution environment. This does
  not establish the historical host's lock ownership.
- `/home/joe` and the prescribed host codegraph executable are absent.
  `aida doctor check codegraph` reports a missing client. Source inspection
  used `rg`; no index was created or modified.
- `Directory::lock` in `aida-cli-lib/src/review_watch.rs` opens through
  `Directory::file` / `open_file` and returns a kernel flock guard. The source
  explicitly describes transient fork inheritance until exec. That source
  comment is not evidence of which process held the incident lock. Rust
  `OpenOptions` is used; no claim of an observed descriptor's close-on-exec
  flags can be made without its host fdinfo.

Finding 2 remains blocked on host incident evidence: capture the affected
lock's device/inode, kernel lock holder PID, process start identity and command,
and matching descriptor/fdinfo evidence for the controller and descendants.
This must distinguish a live controller performing lock-held work from a child
retaining the open file description. If the episode has ended, retained host
diagnostics or an authorized reproduction are needed; a present-day snapshot
cannot establish its historical holder.

Findings 1 and 3 remain open. No source implementation or tests were changed,
because the required investigation-first boundary is unresolved. No lock was
unlinked, stolen, or used as grounds to kill a process. Publication must remain
vetoed pending resolution of the punt.
