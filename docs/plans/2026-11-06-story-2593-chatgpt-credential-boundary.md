# STORY-2593: contained ChatGPT credential boundary sketch

<!-- trace:STORY-2593 | ai:codex -->

Status: proposed; master/operator sign-off required before implementation.
This is the guided design checkpoint, not a completed containment change.

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

Checked 2026-11-06: [Codex authentication](https://learn.chatgpt.com/docs/auth)
documents cached login storage and automatic token refresh.
[Codex app-server external OAuth setup](https://developers.openai.com/siwc/token-sharing-open-source/codex-app-server)
documents an access-token provider flow with host-managed renewal. Neither is
proof that the installed `codex exec` supports the proposed existing-login
handoff. That compatibility check and approval of the residual bearer-token
exposure are prerequisites to implementation.
