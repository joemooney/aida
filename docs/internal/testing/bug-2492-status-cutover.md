# GitHub merge-hold status cutover

<!-- trace:BUG-2492 | ai:codex -->

Owner: **Joe Mooney, repository operator**, runs this procedure at a human
terminal, including the merge attempts and human-gated clearance. The host
launcher publishes the implementation PR; it does not perform this cutover.
This is a deployment verification procedure, **not recorded live evidence**.
Keep its command outputs, run URLs, timestamps and UI screenshots in the
implementation PR and on BUG-2492 before declaring the cutover verified or
marking the spec Completed. The operator runbook is the evidence route; the
implementation and its local tests do not satisfy the live acceptance checks.

The host must publish or retitle the implementation PR with a trailing
`(Refs BUG-2492)`, including the existing PR #3651. A completing `(BUG-2492)`
trailer would close the spec before the post-merge evidence can be gathered.
The native implementer records this subject with `aida ship -m`; the host owns
PR metadata changes and must verify the final title before publication.

## Bootstrap without changing protection

The previous default-branch workflow has only `pull_request`; this PR originally
had only `pull_request_target`. That combination schedules neither publisher
for this PR. This revision retains **both triggers for one release**. The
`pull_request` path runs the new publisher for same-repository branches, including
this implementation PR, using its Actions `GITHUB_TOKEN`. It checks out no code.
Fork `pull_request` jobs are skipped; their required status stays missing until
the trusted default-branch `pull_request_target` workflow is installed. The new
job is named `publish-merge-hold-status`, never `merge-hold-gate`.

1. After the host pushes this revision, Joe checks that its `pull_request` run
   actually publishes `merge-hold-gate` on the **current PR head**, using the
   API snapshot below. If necessary an ordinary label change retriggers it.
2. Verify the app-pinned pending/success behavior on a disposable same-repository
   canary before merging the implementation. A canary based on this revision
   exercises the bootstrap path. Do not hand-post a PAT status: that would not
   establish that `GITHUB_TOKEN` meets the GitHub Actions app pin.
3. Clear the implementation PR only through ordinary authorized clearance and
   merge only when the existing protection accepts its status and all other
   required checks. Keep `merge-hold-gate` pinned to app **15368** and keep
   `enforce_admins=true`. A refusal is a failed verification, not permission to
   remove the pin, disable protection or use an admin bypass.
4. Once merged, execute the target-event tests below. The tracked follow-up
   **TASK-b6b1278890774ef993d2ebc327862470-978** (blocked by BUG-2492) requires a PR
   removing only the temporary `pull_request` trigger, its same-repo job
   condition, and the corresponding bootstrap fixture assertions. Retain
   `pull_request_target` and all its event types. Before merging that cleanup,
   verify that its own label event uses the trusted default-branch publisher.
   Record the removal PR link here and on BUG-2492. Until removal, a
   same-repository PR owns its bootstrap workflow and has `statuses:write`;
   its copy could publish success. Removing the PR trigger removes this bootstrap path. Same-repository PRs
   can still introduce a workflow requesting `statuses: write` and publish the
   same Actions-app status; this pre-existing trust limit requires review of
   workflow changes and is not closed by this cutover.

## Legacy checks and statuses with the same name

GitHub documents that a required name shared by a check run and a commit status
requires **both to pass**. Thus a stale successful legacy `merge-hold-gate`
check run cannot substitute for a pending status of that name. A failed legacy
run can continue to block clearance; it must not be masked by a successful
status. See [GitHub's required-check troubleshooting rules](https://docs.github.com/en/pull-requests/how-tos/merge-and-close-pull-requests/troubleshooting-required-status-checks).
That page separately describes expected-app enforcement. These documented rules
are the basis for the migration; they do **not** prove this repository's app pin
accepts the new publisher. The live checks below are required to establish that.

Before installation, Joe inventories open PR heads and their legacy check runs.
Preserve at least one disposable, unheld canary with an existing successful
legacy Actions check on its head SHA. After installation, apply a hold to that
same SHA and run the collision test below. Do not rerun the old workflow for
this test: that could replace the successful legacy evidence and hide the case
being tested. For PRs with failed legacy checks, Joe arranges an ordinary new
head (merge/update the base or push reviewed work), then waits for fresh CI and
the new publisher. Do not delete checks or weaken protection to unblock them.

## Capture the actual policy and evidence

Set these explicitly at the operator terminal (never use a production PR for
an experimental merge):

```bash
repo=joemooney/aida
pr=CANARY_NUMBER
sha=$(gh api "repos/$repo/pulls/$pr" --jq .head.sha)
gh api "repos/$repo/branches/main/protection" > protection-before.json
gh api "repos/$repo/commits/$sha/status" > status-before.json
gh api "repos/$repo/commits/$sha/check-runs" > checks-before.json
```

Confirm `required_status_checks.checks` contains
`{"context":"merge-hold-gate","app_id":15368}` and
`enforce_admins.enabled` is true. Check any applicable rulesets as well. Save
full responses, not only the combined status `state`: other contexts can affect
that aggregate. Inspect `statuses[]` for the context, state, creator and
`target_url`; inspect `check_runs[]` for name, head SHA, conclusion and `app.id`.
The status API does not itself prove which app satisfied protection. The run
URL, the workflow's `GITHUB_TOKEN` use, unchanged policy and actual merge
refusal/acceptance together supply that evidence.

## Held canary, legacy collision, and label-only release

1. Disable auto-merge on the disposable canary. Have every unrelated required
   check/review settled and make sure the canary meets base freshness policy.
   If another gate blocks, a refused merge alone is inconclusive; record that
   limitation and repeat on a suitable canary. Preserve the legacy-success
   canary separately if updating its head would destroy the collision case.
2. Joe applies a hold through the normal AIDA hold path, retaining the recorded
   label and removing any old clearance. Save the head SHA before changing
   labels. After cutover, verify the Actions run event is
   `pull_request_target` and its workflow revision is the trusted base version.
   Wait in bounded steps for completion, capturing the run URL. This must be
   a label event, with no push and no changed head.
3. Fetch `/commits/$sha/status` and `/commits/$sha/check-runs` again. Require
   the latest `merge-hold-gate` status to be **pending**, and the
   `publish-merge-hold-status` job to succeed. On the legacy canary, also
   require the pre-existing same-name Actions check to remain **success** on
   the same SHA. This is the collision test, not just a clean-head test.
4. On the disposable held canary, execute raw
   `gh pr merge "$pr" --repo "$repo" --squash --match-head-commit "$sha"`.
   Do not pass `--auto` or `--admin`. Save its exit code and complete output;
   require a nonzero refusal attributable to the required pending gate. Check
   the web UI also shows the required gate pending and merge unavailable.
   Do not press an enabled merge button. If either surface permits merging,
   stop the rollout and report the evidence; do not merge real held PRs.
5. Joe runs `aida merge-hold clear "$pr"` through the normal interactive
   clearance path. Require labels `aida:merge-hold-recorded` and
   `aida:merge-hold-cleared`, without `aida:merge-hold`. Save the new
   `pull_request_target` run with event action `labeled` or `unlabeled`.
   Verify `gh api "repos/$repo/pulls/$pr" --jq .head.sha` still equals `$sha`.
   Fetch `/commits/$sha/status` again: the same context on that exact SHA
   must now be **success**, with its URL pointing to the new label-event run.
   The earlier pending record should remain visible in the paginated
   `/commits/$sha/statuses` history. This proves label-only pending-to-success.
6. Check the UI now accepts that required gate. With other gates settled, merge
   the disposable, cleared canary using the same raw command and save success.
   Repeat pending/refusal/label-only-clearance/success on a fresh-head canary
   with **no legacy `merge-hold-gate` check run**. Success here establishes that
   the status alone satisfies app 15368, rather than relying on a legacy run.
7. Save `protection-after.json` and confirm the required context, app pin and
   admin enforcement match the before snapshot. Record run conclusions: holds
   must not produce failed publisher jobs; malformed evidence must still fail.
   Observe the operator's Actions notifications for the held run (no failure
   mail). Local fixtures verify the exit/state split, not mail delivery.

If app attribution, collision behavior or either merge surface fails these
checks, Joe stops the cutover and keeps the verification marked incomplete.
Repair or revert the publisher through a reviewed PR; do not silently change
branch protection. Existing PR holds and human clearance requirements still
apply throughout. There is no claim in this change that the live checks have
already been performed.

## Evidence record required before completion

Round 3 review reported the bootstrap at commit `71d1bbb84088` publishing
`merge-hold-gate=pending` ("Merge held: awaiting recorded clearance"), a successful
`publish-merge-hold-status` job, and `mergeStateStatus=BLOCKED`. This is reviewer
observation, not an independent rerun or evidence that the raw merge and UI
checks passed. It establishes bootstrap/pending behavior only.

The repository operator must attach the procedure's outputs to BUG-2492:

- Canary PR/head and run URLs, unchanged app pin 15368/admin enforcement snapshots.
- Raw merge exit/output and UI refusal with unrelated gates settled, for the
  legacy-success collision and status-only fresh-head cases.
- Label-event run and pending-to-success status history on the unchanged SHA,
  plus cleared-canary merge acceptance and held-run notification observation.

Until those outputs are recorded, AC1/AC2 live enforcement remains unverified
and BUG-2492 must not be marked Completed. No protection weakening or merge
attempt is authorized from an implementer seat by this runbook.
