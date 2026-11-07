# STORY-2593: contained ChatGPT credential boundary sketch

<!-- trace:STORY-2593 | ai:codex -->

Status: option (a) signed off by the advisor on 2026-11-06 (recorded on
STORY-2593). This is the design record, not a completed containment change.
This file is the canonical design; the STORY-2593 comment points here rather
than maintaining a second copy.

## Round 2: acceptance and approval checkpoint

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
STORY-2593 on 2026-11-06; implementation proceeds under its conditions.

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
