# STORY-2593: contained ChatGPT credential boundary sketch

<!-- trace:STORY-2593 | ai:codex -->

Status: option (a) signed off by the advisor on 2026-11-06 (recorded on
STORY-2593), but the step-1 compatibility gate failed on 2026-11-06.
Implementation is blocked pending an advisor/operator interface decision on
the ID-token-shaped/empty-refresh schema or another supported `codex exec`
interface. This is the design record, not a completed containment change.
This file is the canonical design; the STORY-2593 comment points here rather
than maintaining a second copy.

## Step-1 evidence: signed interface gate fails (2026-11-06)

The advisor's spec comment at `2026-11-07T00:33:24Z` signs off option (a),
accepts the readable bearer-token risk, and makes the installed `codex exec`
compatibility probe a hard gate. Finding 5's missing-sign-off premise is now
superseded. This does not satisfy AC1-AC3. That signed comment assigns steps
1-3 to the implementation PR and reserves steps 4-5 (the live C0-C4 run and
BUG-2447/2448 disposition) for the advisor through deputy 3. The later scope
comment at `2026-11-07T00:45:56Z` moves AC1-AC3 to the implementation PR;
it does not reassign ownership of steps 4-5. The design record landed as
`90d0ec0baf4` (#3544), with a `docs(plans)` subject and
`(Refs STORY-2593)` trailer; that squash resolves finding 4's
completion-metadata concern for the landed record.

Synthetic probes used the installed `codex-cli 0.166.0` executable at
`@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex`, SHA-256
`b8a11fedf24b2dfb59a70c9e52b030f9a508dcd49cd2dd0d41956f4c785156b8`.
Each case had a fresh temporary HOME, CODEX_HOME and XDG directories, an
explicit environment without inherited credentials, and fake tokens only.
No operator auth file or keyring was read.

Fixtures used `auth_mode: "chatgpt"`, a `tokens` object with the fake access
token and account ID, and a recent `last_refresh`. The synthetic ID token
was an unsigned JWT (`alg: none`, `exp: 4102444800`) whose
`https://api.openai.com/auth` claim named the same fake `chatgpt_account_id`
and `chatgpt_plan_type: "plus"`; it was not copied from a real login.

| Synthetic input / command | Observed result |
| --- | --- |
| `login status`, access token plus account ID only | Exit 1: missing `id_token`. |
| `login status`, synthetic ID token plus access token and account ID, no refresh field | Exit 1: missing `refresh_token`. |
| `login status`, same input plus `refresh_token: ""` (control only) | Exit 0: logged in using ChatGPT. |
| `exec`, no refresh field | Local `/v1/responses` request carried neither the expected bearer nor account ID. |
| `exec`, empty refresh string (control only) | Local `/v1/responses` request carried both the expected bearer and account ID. |

The `exec` probes used a custom Responses provider with
`requires_openai_auth=true`, WebSockets disabled and zero request/stream
retries. Its base URL was an ephemeral loopback HTTP recorder; other HTTP(S)
traffic was pointed at an unavailable loopback proxy. Both requests received
a deliberate HTTP 400 and `exec` exited 1. That exit is an expected fixture
response, not a successful inference or containment run. Each child had a
seven-second timeout. Neither stdout nor stderr contained the fake access
token, and each input auth.json was unchanged. This checks only those output
streams and the input file, not every possible runtime/log artifact.

**Stop at step 1.** The approved leaf explicitly requires no refresh field;
the installed cached-ChatGPT path did not authenticate that shape. The control
shows a possible alternative requiring an ID-token-shaped value and an empty
refresh field. It does not authorize changing the approved leaf contract,
prove compatibility with real account/workspace restrictions, or establish
absence of secret persistence. Return this distinct interface decision to
the advisor as required by the sign-off; do not substitute app-server, copy
host refresh material, or remove `refuse_chatgpt()`.

Findings 1-2 remain open: production authentication and credential visibility
sentinels are unimplemented; no ChatGPT C0-C4 run, policy hash or containment
receipt is supplied. Finding 3 remains open: BUG-2447 and BUG-2448 currently
show Approved and remain blocked by this story; no C3/C4 evidence supports
closing or rescoping them. Their historical Needs Attention labels below are
round-2 observations. The punt for this interface gate vetoes publication and
completion; these local evidence edits are a handoff, not another completed
implementation slice.

## Decision escalation and host handoff

Review of `0c3299c77788` is accepted on its facts. Findings 1-3 remain open:
the production refusal is retained, containment and credential-visibility
evidence is absent, and BUG-2447/2448 remain Approved and blocked. Treating
those findings as actionable implementation work before resolving finding 5
is disputed: the signed step-1 stop condition requires an advisor/operator
interface decision first. Accepting the ID-token-shaped/empty-refresh schema
or naming another supported `codex exec` interface needs an explicit recorded
decision; requeueing without one does not resolve the gate.

Finding 4 remains a host action. Commit `0c3299c77788` uses docs/Refs, but
`14ef038e232f`, `e58b7e8cafc` and PR #3560 still claim feature completion.
The corrected latest commit does not correct the PR's squash title.
Future commits on this branch must use the docs/Refs subject below.
Before any eventual squash, the host must retitle
the PR to
`[AI:codex] docs(plans): STORY-2593 step-1 interface-gate evidence (Refs STORY-2593)`
or close it unmerged. This native seat cannot change Git metadata or perform
PR operations. The merge hold and interface-gate punt remain publication and
completion vetoes; a corrected subject does not clear either gate.

Finding 6 is corrected: the ownership paragraph is reflowed and section
headings use descriptive names without review-round numbers. Escalate finding
5 to the advisor/operator instead of scheduling another docs-only implementer
round. Record the chosen credential representation and its security and
compatibility requirements on STORY-2593 before resuming steps 1-3. Requeueing
without that decision leaves the signed stop condition unresolved. After that
decision, implementer steps 1-3 precede advisor/deputy-3 steps 4-5; none is
completed here.

## Historical acceptance and approval checkpoint

Review of `058e9899a195` correctly found that all three acceptance criteria
remain unmet. This revision is documentation only:

| Finding | Current evidence and disposition |
| --- | --- |
| 1 / AC1 | `refuse_chatgpt()` still rejects an existing auth.json and the launcher still requires OPENAI_API_KEY. No brokered ChatGPT path is implemented. Deferred behind the design approval below. |
| 2 / AC2 | No C0-C4 ChatGPT run under `--boundary aida`, credential-visibility probes, or policy/binary/receipt hashes are supplied. They remain required after implementation; absent evidence is unknown. |
| 3 / AC3 | BUG-2447 and BUG-2448 remain needs-attention and blocked by this story. No new C3/C4 measurement justifies closure or rescoping. |
| 4 / metadata | The prior feat subject and completion trailer overstate the diff. Keep PR #3544 open and held; it must not complete this story as a sketch. Host follow-up must correct the PR title before any eventual publication. |
| 5 / guided gate | Queue rework returned the story to Approved without a triage reason; that is not recorded approval of the credential design. Implementation steps 1-5 remain gated. |
| 6 / references | Both original URLs resolve; the authentication entry point redirects as documented below. Reference reachability does not establish installed CLI compatibility. |
| 7 / duplication | Maintain the design here; replace the original full-text spec comment with a pointer. |

Suggested docs-only host commit/PR subject, if publication is later authorized:
`[AI:codex] docs(plans): clarify ChatGPT credential design gate (Refs STORY-2593)`.
This is a handoff recommendation, not publication authorization. A punt or hold
remains a publication veto, and the existing historical commit is not rewritten
from this native seat. The merge-hold CI failure requires the authorized review
and clearance path; it is not a failing implementation test to bypass.

The master/operator must record explicit sign-off on STORY-2593 for option (a),
including the residual readable bearer-token risk, the no-in-lane-renewal
deadline, and the requirement to return to this gate if installed `codex exec`
cannot consume access-only credentials. The advisor recorded that sign-off on
STORY-2593 on 2026-11-06. Its step-1 stop condition was subsequently reached;
steps 1-3 remain blocked pending the interface decision described above.

## Observed implementation

`aida-cli-lib/src/launch/contained.rs` resolves the host Codex home, calls
`refuse_chatgpt()` whenever `auth.json` exists, then requires `OPENAI_API_KEY`.
The shared launcher already supplies a private vendor home and masks the
default and relocated host vendor-state roots. Claude's descriptor-pinned,
read-only credential leaf is a separate vendor-specific mechanism; its expiry
schema must not be reused for Codex.

The AIDA conformance fixture preserves the original Codex home for production
authentication and isolates HOME/AIDA_HOME. Its current C1/C2 probes concern
writes, not credential confidentiality. A successful write-denial result cannot
establish that refresh tokens are invisible.

No credential contents were read during preparation of this sketch. The owning
story had no existing comments or design approval at pickup.

## Proposed decision: host broker, access-token-only delivery

Choose option (a), subject to validating the installed Codex CLI interface.
The host retains the refresh token and serializes refresh for the existing
login. Only an access token and necessary account routing metadata cross into
the lane. Do not claim that copying an access token narrows its upstream scope
or lifetime: those properties require provider support. Lane authorization
expires separately and cannot remotely invalidate a leaked bearer token.

Before implementation, prove with synthetic credentials and the installed
Codex version that the existing `codex exec` launch can use external access-only
auth without a refresh token, preserve ChatGPT account routing, and avoid
persisting secrets to logs. Record the exact supported interface/version in
tests. If that fails, return to this gate; do not silently replace `exec` with
app-server or introduce a new login. Current app-server documentation describes
an external OAuth-token provider path, but does not establish compatibility
with the operator's existing CLI login or this launch surface.

The host opener must pin a bounded, non-symlink, singly linked regular credential
file owned by the invoking user with private permissions. Refuse malformed,
unsupported, missing-expiry or expired evidence without echoing secrets. Keyring
logins need an explicitly supported host reader or a named refusal, never a
worker keyring mount. Respect account/workspace restrictions and serialize with
the supported host auth writer; do not invent a second refresh writer against
the operator's rotating login. Until that integration is demonstrated, refuse
refresh and require host reauthentication/relaunch when validity is insufficient.

Deliver access-only material through a private, descriptor-backed memory leaf
bound read-only into the lane's private CODEX_HOME, if the verified interface
supports that format. Keep generated config and credential leaves read-only
while vendor runtime state remains writable. Deny fallback to host auth,
inherited alternate credentials, host keyrings, and refresh material. Never
copy the whole auth cache. The worker can read its supplied bearer token; this
is an explicit residual access-token exfiltration risk, not secret isolation
from arbitrary terminal tools inside the lane.

Start with no in-lane renewal: require validity through the lane grant plus a
documented margin, bound execution to that deadline, and fail closed with a
relaunch diagnostic. Longer lanes need a separately reviewed renewal protocol.
On every launch failure, child exit, cancellation or grant expiry, terminate
the lane, close memory handles and discard private state. Crash cleanup must
not leave a disk-backed credential copy. This revokes local delivery only;
upstream logout/revocation remains an operator action and must not log out the
shared host session automatically.

## Egress and authority

Retain the existing mandatory connection broker and host-attested receipt.
`auth.openai.com` and `chatgpt.com` are already in the trusted allowlist, along
with `api.openai.com`. This proposal adds no wildcard hosts or direct-network
bypass. Host refresh is a separate host-side operation; allowing an auth host
does not give the worker a refresh credential. Account-bearing requests still
require the existing destination enforcement. No new listening TCP port is
needed for the initial access-only handoff.

Do not enable additional contained launch surfaces or change native defaults.
Do not give credential delivery any store, Git, reviewer or publication authority.

## Alternatives requiring a different approval

Option (b), a whole auth.json copy even on read-only tmpfs, exposes reusable
refresh material to all lane tools and permits exfiltration. Read-only storage
does not prevent token use or rotation elsewhere; deleting it does not revoke
stolen tokens. It is not the recommended choice.

Option (c), API-key-only operation with an operator-provisioned key, preserves
the current bounded mechanism but does not meet this story's existing-ChatGPT-
login acceptance criterion. It requires explicit rescoping rather than claiming
completion or closing BUG-2447/BUG-2448.

## Implementation and validation after approval

1. Pin the installed-version auth contract with fake-token tests; implement
   the approved host reader and access-only delivery in the shared launcher.
   Keep the existing refusal until every new preflight succeeds.
2. Test malformed/oversized/symlink/hardlink/FIFO/unsafe-permission inputs,
   expired/too-short lifetimes, unsupported storage, launch rollback and lane
   teardown. Ensure diagnostics, receipts and reports contain no token values.
3. Add synthetic credential sentinels to conformance: C1-associated visibility
   evidence must show host default/relocated homes, keyring endpoints and refresh
   sentinels unreadable through terminal and file tools. Assert the intended
   access-only leaf is readable and immutable, contains no refresh field, and
   host auth is unchanged. C2 must retain Git/store authority probes and verify
   no credential artifacts enter those writable areas. Report visibility
   separately from existing write statuses; missing evidence stays unknown.
4. Run launcher regressions and the fast conformance suite. Then a host operator
   runs C0-C4 under `--boundary aida` with the existing ChatGPT login and current
   binary, retaining policy/binary/receipt hashes and redacted diagnostics.
   This managed seat must not fabricate TTY authority or fall back to native.
5. Use actual C3/C4 evidence to close or rescope BUG-2447/BUG-2448. Authentication
   success alone supplies no containment pass credit.

## Official reference and remaining uncertainty

Rechecked 2026-11-06 by opening both official pages:
[Codex authentication](https://developers.openai.com/codex/auth) redirects to
[Authentication](https://learn.chatgpt.com/docs/auth). Its "Login caching" and
"Credential storage" sections document file/keyring storage and automatic
ChatGPT token refresh. The redirect explains the original Learn URL.
[Codex app-server external OAuth setup](https://developers.openai.com/siwc/token-sharing-open-source/codex-app-server)
also resolves. Its setup supplies an application-owned access token through an
environment-backed Responses provider, with application-managed renewal and
process restart. That separate app-server integration does not demonstrate
reuse of the operator's CLI login. Neither page proves that the installed
`codex exec` supports the proposed existing-login handoff. That compatibility
check and approval of the residual bearer-token exposure remain prerequisites
to implementation.
