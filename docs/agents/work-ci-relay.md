# Restricted work CI relay

<!-- trace:STORY-2480 | ai:codex -->

Home assigns a positive `u32` attempt number. A request is an immutable,
permanently retained `refs/aida/ci/<attempt>` pointing directly to the exact
commit on both the home hub and work GitLab. The advisor's 2026-11-05 ruling
supersedes the earlier `refs/heads/ci/<attempt>` admission proposal. There is no
request registry. Never move, rename, delete or recycle an attempt; pruning is
out of scope. A partial request publication retries the same attempt and SHA. The request
command marks the native check pending before publishing the retained work ref;
uncertain pending-status writes require native evidence before replay.
Fresh clones recover authority from these refs, rather than relay journals.

The work user timer makes outbound connections only. One `intent ci poll` pass
handles one pending attempt. It verifies matching retained home/work refs,
fetches the exact home commit into an independent object database, and creates
`refs/heads/ci/<attempt>` on work to trigger GitLab's push pipeline. This branch
is an execution projection; it cannot grant request authority. The poller does
not POST new pipelines: uncertain registration waits until the original deadline,
so restarting cannot launch duplicates. More than one matching pipeline refuses
for manual association recovery. Configure work GitLab to run push pipelines on
these execution branches, with no separate MR pipeline for the same request.
Custom `refs/aida/ci/*` refs alone do not trigger a GitLab branch pipeline.

## Server enforcement and credentials

Use separate home read, home request-creation, home intake-return, work Git/API,
and home status/API identities. Work receives no home API/status credential.
Home status credentials stay in the home processor's environment. SSH host
aliases can select distinct keys; Git credential helpers, ambient Git config,
askpass, URL rewriting, hooks and implicit refspecs are disabled in the relay's
independent repositories. HTTPS Git remotes support anonymous access only.

Install [the example pre-receive policy](../../scripts/ci-relay/pre-receive.py)
on the receiving host, or enforce equivalent rules using hosting controls.
A trusted SSH forced-command wrapper must set `AIDA_CI_PRINCIPAL` from the
key's enrolled identity, **never from client environment or push options**:

- `request`: create only canonical `refs/aida/ci/<attempt>` commit refs;
  replacements and deletions refuse, including fast-forward replacements.
- `return`: update only `refs/heads/aida-intent`, never delete or rewind it.
  Arbitrary branch and tag creation must refuse too.
- `mirror`: create only immutable `refs/heads/ci/<attempt>` commit refs at work.

This example is for dedicated CI credentials. Integrate it with the hosting
platform's authenticated identity and existing receive policy; do not expose a
client-selectable identity or give these credentials an unrestricted fallback.
Plain wildcard branch protection is insufficient if the credential can create
other branches or tags. GitLab deployments without per-identity controls/custom
hooks that implement this policy must refuse deployment. Do not assume all
GitLab editions support the example installation.

Enrollment performs real forbidden branch and tag pushes with the enrolled
return destination/credential; both must refuse, followed by successful read
probes. If a forbidden push succeeds, enrollment refuses and reports that the
operator must remove the unexpected probe ref and repair the server. This probe
is a point-in-time check, not permanent proof of server policy. Independently
test request replacement and deletion refusal with the request identity before
starting the timer, as the local integration fixture does. The test requires no
production credential. Never run destructive policy tests against real requests.

## Enrollment and operation

Use dedicated clones at home and work. First enroll the shared intake destination
with `aida intent enroll --remote <return-url> --reviewed`. This is the existing
intake transport and lock, independent of store transport. Bootstrap the first
retained request using the home request identity before CI enrollment, since
live return-credential probes need a source commit. For example, from the home
source clone (with appropriately scoped credentials for each destination):

```bash
git push <home-request-url> <full-sha>:refs/aida/ci/1
git push <work-request-url> <full-sha>:refs/aida/ci/1
```

Never use `--force`. Review a profile JSON file, then run:

```bash
aida intent ci enroll --file ci-profile.json --reviewed
# Home only; explicitly allocate a previously unused attempt:
aida intent ci request --attempt 2 --sha <full-sha>
# Work only; each pass returns instead of waiting:
aida intent ci poll
# Home only; schedule this on the home host:
aida intent ci process
```

A work profile contains exactly these fields:

```json
{
  "version": 1,
  "role": "work",
  "home_remote": "ssh://home-read/aida.git",
  "work_remote": "ssh://work-ci/aida.git",
  "work_api": {
    "url": "https://work.example/api/v4",
    "project": "123",
    "token_env": "WORK_CI_TOKEN",
    "forge": "gitlab"
  },
  "home_api": null,
  "timeout_seconds": 3600,
  "max_polls": 120
}
```

At home set `role` to `home` and supply `home_api` in the same endpoint shape.
GitLab projects use positive numeric IDs; GitHub uses `owner/repository` and
`forge: "github"`, with `url: "https://api.github.com"`. The home clone's remote
credentials must allow creating retained home/work request refs. The work clone's
home remote uses read-only credentials; its work remote uses the mirror identity.
The profile stores credential **environment variable names**, never token values.
API URLs require HTTPS; loopback HTTP is allowed for isolated tests. Redirects
refuse, API responses are capped at 1 MiB, and calls time out after 15 seconds.
Enrollment is immutable; use a separate clone to change a profile.

Copy [the service](../../contrib/systemd/aida-work-ci.service) and
[timer](../../contrib/systemd/aida-work-ci.timer) to `~/.config/systemd/user/`,
adjust their clone/binary/environment paths, and enable with
`systemctl --user enable --now aida-work-ci.timer`. Store the work API token in
the owner-only environment file. No listener or inbound home-to-work connection
is required by the work poller. Home request creation explicitly pushes the
retained request to work under the advisor's ruling.

## Supported inventory and receipt boundary

Both validators derive inventory from the retained home commit, never work API
claims or executable discovery. `.gitlab-ci.yml`, when present, is authoritative.
Supported GitLab definitions are flat static jobs with literal names and `script`;
ordinary stage/image/tags/script/variables/services/cache/artifacts/retry/timeout/
needs/dependencies settings are accepted. Includes, hidden jobs, extends,
parallel/matrices, conditional rules and unknown job keys refuse. If there is no
GitLab definition, static `.github/workflows/*.yml|yaml` job definitions can
supply inventory; matrices, reusable workflows, dynamic names, conditions and
unknown job keys refuse. This does not translate GitHub workflows to GitLab;
the operator must arrange a work pipeline with exactly the pinned job names.
The relay never invents a pipeline definition.

Literal Rust `#[test]` / `#[tokio::test]` function names in committed regular
`.rs` files form the supported failing-test inventory. Names are unqualified
function identifiers, matching the API's `name` field. Inline modules are walked;
all committed `.rs` files are inspected without running code. Dynamic/generated,
parameterized and non-Rust test names are unsupported and refuse if returned.
This profile does not promise discovery of every generated or conditionally
compiled test. There is no test-discovery subprocess or work-supplied inventory.
Source reads are bounded (1 MiB/file, 4096 Rust files, 16 MiB total, 20 seconds).
Unsupported existing repository workflows require a separate supported pipeline
configuration before rollout; no real runner deployment is claimed here.

Receipts use exactly `version`, `kind`, `sha`, `attempt`, `status`, `jobs`,
`failing_tests`, `digest`. Kind is `ci-result`, version is 1. Jobs contain exactly
`name`, `status`, `duration_seconds`. Statuses are `success`, `failed`, `canceled`,
`skipped`. Jobs and failing tests are sorted, unique and allow-listed. Missing jobs
refuse. Success requires every expected job to succeed and no failing tests.
Non-success aggregate precedence is failed, canceled, skipped. API duration claims
are rounded up to integer seconds locally; wire durations accept only integers
0–86400. Limits are 128 KiB, 256 jobs, 2048 failing tests, 512 UTF-8 bytes per
identifier. Duplicate, unknown or missing JSON fields and trailing input refuse.
The SHA-256 digest hashes compact UTF-8 JSON with the eight payload fields in the
listed order, excluding `digest`, using serde_json escaping.

Immutable bytes live at `ci-results/<sha>/<attempt>.json` on `aida-intent`.
Existing observation and processing-receipt formats stay unchanged. Ordinary
intake pulls/publications validate CI history too, so any clone reading that
history must enroll CI admission access. Invalid input remains in the independent
quarantine ref/object database; it cannot advance the accepted intake branch or
publish a native status. No work logs, URLs, paths, runner identities or diagnostic
prose enter the return schema. Allow-lists and digests cannot establish that work
claims are true or eliminate covert channels through durations or statuses.

## Recovery and merge policy

Retry state lives under the existing clone-common `aida-intake/` private directory;
source objects use `ci-source/`. The shared intake lock serializes polling,
publication and home processing. A local journal binds attempt/SHA, original
absolute deadline, poll count and exact pipeline ID **before** side effects.
Pending receipt bytes are persisted in bounded owner-only chunks before push;
restart retries those exact bytes. Concurrent intake publication reuses the
existing bounded fast-forward retry path; it never force-pushes or overwrites
immutable files. Expiration publishes no success and never replenishes its
budget. Retry with a fresh home-assigned attempt instead of editing a journal.

The home processor publishes one stable native check, `aida/work-ci`. Configure
that exact context as required in the home merge/train policy. Local transport
configuration cannot change protected-branch or required-check settings. A newer
admitted attempt supersedes older receipts for the same SHA, even while its
result is pending; old receipts cannot overwrite it. The request command marks the native check pending before sending a new attempt
to work. For the manually bootstrapped first attempt, rerun the matching
`intent ci request` after enrollment and before starting the work timer to
establish that pending status. Merge policy must wait for the requested attempt.

Processing markers bind the exact receipt digest. A fresh home clone or a restart
after successful POST reconciles native status context, state and a home-authored
attempt/digest description before considering another POST. Status reads have a
bounded 100-row page; overflow refuses. A pending POST with no confirmed matching
status remains uncertain and refuses automatic replay. Inspect the forge and
owner-only `ci-status-pending-<attempt>.json`; clear that marker only after proving
that the original writer stopped and no status was accepted. Keep journals when
recovering failed or ambiguous writes. No receipt selects the endpoint, repository,
credential or status context.

Validation:

```bash
cargo test -p aida-cli-lib --no-default-features ci_receipt::tests --lib
cargo build -p aida-cli --no-default-features
AIDA_TEST_BIN="$PWD/target/debug/aida" python3 tests/test_work_ci_relay.py
```
