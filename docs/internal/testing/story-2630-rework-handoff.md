# Host publication handoff

<!-- trace:STORY-2630 | ai:codex -->

Apply the following text to PR #3662 before re-review; an ignored seat draft
is not the PR body. Existing punt/hold signals remain publication vetoes.

Rework deployment guidance for the host-query proxy (Refs STORY-2630).

Against origin/main 08cbf8a3fe: AC1 primary-index link routing is present, but
selection of the proxy client remains a host deployment task. AC3 template
master/shared navigation guidance is present. AC4 missing/stale/unreachable
findings are present; this branch adds proxy snapshot age reporting. AC2 and
AC5 remain OPEN: this is a Refs slice, not a completing implementation.

This revision corrects the README to native-sandbox scope, uses the explicit
host-owned client for both existing and new seats, and records the missing
AIDA-contained filesystem admission. No index write grant is added.

Exact native-seat observations, 2026-11-08:
$ codegraph callers primary_index_link --json
/bin/bash: line 10: codegraph: command not found
$ aida doctor --lane-host --json
error: unexpected argument '--lane-host' found

Usage: aida doctor [OPTIONS] [COMMAND]

For more information, try '--help'.

Neither .codegraph nor /home/dirtlab/ai/aida/.codegraph exists. No successful
callers result or live read-only grant verification can be claimed here.
The seat's primary checkout is outside its writable roots, but that alone is
not AC2 evidence for an absent index or an AIDA-contained mount.

Host operator Joe owns: provisioning/indexing on the primary; installing the
reviewed host proxy outside seat write roots; delivering its absolute client
path to lanes (bare codegraph is still not routed); read-only admission for
client/descriptor/socket in AIDA-contained posture; running the contained-seat
callers query and candidate doctor --lane-host; pasting their exact successful
output and actual grant evidence into the PR body before re-review/completion.
Do not run index/init/sync inside the lane. Do not complete STORY-2630.

Review finding 5 remains open: serialized queries/backups can exceed doctor's
250ms probe deadline. Host-proxy follow-up must make status responsive under
load before final acceptance. This handoff does not claim to fix that finding.
CI at the new head is a host publication/review requirement, not a local claim.

Local validation for this documentation revision:
- Python proxy suite: 12 tests, 11 passed, 1 skipped (CodeGraph absent).
- git diff --check: passed.
- make check-templates: reached portable .agents drift scan; no final result
  available within the lane tool lifetime. Host must rerun.
- No Rust changes or Cargo execution in this revision.
- Punt recorded Needs Attention; separate punt signal write failed with
  read-only filesystem. The recorded punt remains a publication veto.
- PR text also added as a STORY-2630 comment; host must apply it to PR #3662.

Changed files: scripts/codegraph/README.md and this handoff. All edits are
uncommitted. No commit, push, PR edit, queue completion or ship was performed.
