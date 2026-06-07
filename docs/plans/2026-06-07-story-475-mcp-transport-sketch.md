# Design sketch: remote/auth-capable AIDA MCP transport + threat model

**Date**: 2026-06-07
**Specs**: STORY-475 (Remote/auth-capable AIDA MCP transport), pairs with STORY-474 (MCP tool profiles, shipped)
**Status**: Sketch — NOT approved for implementation. Operator security sign-off required before any code.
**Complexity**: High (new network + auth + audit surface; security-sensitive)

> **Scope discipline.** This document is a design sketch and threat model only.
> It contains no transport code, no network code, no auth code. STORY-475 is
> DEFERRED by operator decision (2026-06-06) until post-stability *and* a
> concrete remote/cloud-agent need exists. The High-priority tag predates that
> decision. This sketch exists so that when the need is concrete, the security
> conversation starts from a written threat model rather than a blank page.

---

## 1. Starting point — what exists today

`aida mcp-serve` is **stdio-only** (`run_mcp_server` in `aida-cli/src/mcp.rs`):
JSON-RPC frames are read line-by-line from `stdin` and written to `stdout`.
The transport is implicitly authenticated by the OS process boundary — only a
process that the operator's MCP client spawned can talk to it, and it inherits
that process's filesystem identity and cwd. There is no network listener, no
token, no caller identity beyond "whoever launched the process."

Two security primitives are already in place and are the load-bearing reuse
points for any remote transport:

- **Tool profiles (STORY-474, shipped).** `McpProfile` (`read-only` →
  `coordination` → `operator` → `admin`/`full`) gates the tool surface at BOTH
  `tools/list` (out-of-profile tools are not advertised) and `tools/call`
  (out-of-profile tools are rejected with a `permission_denied` envelope even
  if called by name). Resolution order: `AIDA_MCP_PROFILE` env → `[mcp] profile`
  in `.aida/config.toml` → `full`. See `tool_in_profile`, `tool_min_profile`,
  `resolve_mcp_profile`, `tool_descriptors_for_profile`.
- **Project scoping.** The server's root is the cwd/project where `mcp-serve`
  was started; all reads/writes resolve against that single project's store.

The design principle for STORY-475 is therefore: **a remote transport must not
be able to expose a larger surface, a wider project scope, or a weaker identity
guarantee than stdio already provides.** The transport changes *who can reach
the server*; it must not change *what the server is willing to do*.

---

## 2. Transport options + trade-offs

The decision is not "which transport is best" in the abstract — it is "which
transport gives the smallest new attack surface for the first concrete need."
The MCP spec recognizes stdio and Streamable HTTP (the successor to SSE) as the
standard transports; everything below is framed against that.

| Option | What it is | Pros | Cons / risk | Verdict for v1 |
|---|---|---|---|---|
| **A. stdio (today)** | Process pipes | Zero network surface; OS process boundary is the auth; no listener to attack | Local same-host only; can't serve a cloud agent | Keep as default forever |
| **B. Local-loopback HTTP + bearer token** | `--transport http --bind 127.0.0.1:<port>`, `Authorization: Bearer <token>` | Smallest network step; loopback is not routable off-host; lets a *local* cloud-agent bridge / port-forwarder connect; token is the only new secret | A token now exists and can leak; any local process can reach the port (loopback is not a strong boundary against co-resident processes); needs constant-time token compare + audit | **Recommended v1 substrate** (read-only profile only) |
| **C. SSH tunnel over (B)** | Operator runs B on loopback; remote reaches it via `ssh -L`/`ssh -R` | Reuses SSH's mature auth (keys, known_hosts, MFA); AIDA writes no network auth code; encryption + identity are SSH's problem; revocation = revoke the SSH key | Operational burden on the user; doesn't help a hosted cloud agent that can't open an SSH session to the operator's box; tunnel endpoint still lands on the loopback port so B's local-process risk remains | **Recommended deployment pattern** for "real remote" without AIDA owning crypto |
| **D. mTLS HTTP** | TLS with required client certificates | Strong mutual identity; encryption; revocable via CRL/short-lived certs; binds caller identity into the transport (good for audit) | AIDA must own cert issuance/rotation/validation — large, error-prone surface; PKI is a project on its own; overkill for loopback | Defer to a later phase if non-loopback bind is ever required |
| **E. OAuth 2.1 / Dynamic Client Registration** | The MCP-spec auth path for hosted remote servers | Standards-track for marketplace/hosted MCP; delegated identity; scopes map cleanly to profiles | Heaviest; needs an authorization server or a trusted IdP; redirect flows; token lifecycle; only worth it for a *hosted* AIDA MCP, which is far beyond current need | Spike only; explicitly out of v1 (matches Codex sketch point 3) |
| **F. Bind to `0.0.0.0` / LAN / public** | HTTP listener on a routable address | Direct reachability | Largest blast radius; one config mistake exposes the substrate to the network; should never be a default and arguably never a documented option without mTLS+auth | **Forbidden in v1**; gate behind an explicit, scary opt-in if ever |

**Layering insight.** B + C compose: AIDA only ever owns a loopback bearer-token
HTTP server (B), and "remote" is achieved by tunneling that loopback port over a
channel whose auth/crypto AIDA does not write (C = SSH today, a cloud provider's
private networking later). This keeps AIDA out of the crypto/PKI business in v1
while still serving a remote caller. The expensive options (D, E) become real
only if/when AIDA must bind to a routable address itself.

---

## 3. Auth model

### 3.1 Who may read / write the requirement graph remotely

| Capability | v1 rule |
|---|---|
| **Read** (list/show/search/graph/history) | Allowed over HTTP only when a valid bearer token is presented AND the active profile is `read-only`. This is the smallest-safe-slice (§5). |
| **Coordination writes** (comments, findings, punts, claims, briefs) | Requires a valid token AND an explicitly-selected `coordination` profile AND explicit operator opt-in per-project. NOT a v1 default; gated behind sign-off. |
| **Spec-graph writes** (`add_requirement`, `update_requirement`) | `operator` profile. NOT exposed over HTTP in v1 at all. Local-stdio only until auth maturity is proven. |
| **admin/full** | NEVER exposed over a network transport. Local stdio only, period. |

The hard rule, restated: **HTTP transport without a token may do nothing.
HTTP transport with a token may do only what its profile permits. Write-capable
profiles over HTTP require explicit operator opt-in and are out of the v1 slice.**
This mirrors the checklist requirement "Remote MCP transports require
authentication before exposing write tools."

### 3.2 Token model

- **Format**: a high-entropy random opaque token (≥256 bits, URL-safe base64),
  not a JWT in v1 (no claims to verify, no key management). Compared in
  constant time. No structure the server parses = no parser attack surface.
- **Provisioning** (first match wins, mirroring profile resolution):
  1. `--token-env AIDA_MCP_TOKEN` — server reads the token value from the named
     env var. The env var holds the secret; the flag holds only the var *name*,
     so the secret never lands in `ps`/argv/shell history.
  2. A generated one-shot local token file under `.aida/` (mode `0600`,
     gitignored by the deny-by-default `.aida/*` rule — confirm `!.aida/<name>`
     is NOT added for it). `aida mcp-serve` could print "token written to
     .aida/mcp-token (0600)" to stderr on first run.
- **Never**: token in argv, token in committed config, token in the audit log,
  token echoed to stdout (stdout is the JSON-RPC channel), token in any
  user-facing error string.
- **Rotation/revocation**: revoke = delete/replace the token file or unset the
  env var and restart the server. No server-side session store in v1, so there
  is nothing to invalidate beyond the single shared secret. (A per-client token
  table is a later-phase item, needed before multi-client HTTP.)

### 3.3 Scope / permissions

Scope = **profile ∩ project-root ∩ token-validity**. There is no per-tool ACL
beyond the profile tiers in v1 — profiles already are the permission model, and
reusing them (not inventing a parallel one) is the point. The three orthogonal
gates a remote call must pass:

1. **Token valid?** (constant-time compare) — else `401`-equivalent envelope.
2. **Tool in active profile?** (existing `tool_in_profile`) — else
   `permission_denied` (existing path).
3. **Target path inside project root?** (existing project scoping) — else
   reject; never traverse outside the cwd project. No multi-project HTTP daemon
   in v1 (one server = one project root).

---

## 4. Threat model

Assets, ranked by what an attacker most wants:
**A1** the requirement graph contents (may contain unreleased strategy, customer
names, security-relevant spec text); **A2** write access to the graph (poison the
substrate that drives autonomous drains); **A3** the audit log integrity; **A4**
the operator's host (RCE / lateral movement via the listener); **A5** the token
itself (key to A1/A2).

Trust boundaries: the new HTTP listener is the boundary. Below it, the same
process identity and filesystem access as today. So the worst case for the
substrate is bounded by *what the server process can already do locally* — the
transport's job is to make sure only the right caller can drive it.

| # | Threat | Vector | Worst case | Mitigation (v1) |
|---|---|---|---|---|
| T1 | **Unauthenticated read** | Listener reachable, no/weak token check | Full graph exfiltration (A1) | Token required before any tool runs; constant-time compare; loopback bind so off-host can't even reach it |
| T2 | **Token theft** | Token in argv/log/env leak, or read off-disk | Attacker gains the caller's full (read-only in v1) capability (A5→A1) | `--token-env` (name not value in argv); `0600` token file; never log token; short-lived rotation guidance; loopback limits remote reach even with a stolen token |
| T3 | **Substrate poisoning** | A write tool exposed over HTTP + token compromise | Adversary edits/creates specs → autonomous drain implements attacker-chosen work (A2) — the highest-impact attack given AIDA drives agents | v1 exposes read-only profile only; write profiles need explicit opt-in + sign-off; admin/full never networked |
| T4 | **Local co-resident process** | Any process on the host hits the loopback port | Same as T1 if it also has the token | Loopback is *not* a trust boundary against co-resident processes — token is the real gate; document that the host must be trusted; consider peer-cred check (SO_PEERCRED on a UDS variant) as a hardening follow-up |
| T5 | **Path traversal / project escape** | Crafted args targeting paths outside the project root | Read/write of arbitrary repo files via the server's identity (A4) | Existing project scoping + explicit reject of out-of-root paths; fuzz/test the arg parsers |
| T6 | **Request flooding / DoS** | Unbounded connections/requests | Server pegged; drain coordination stalls | Connection cap, request size cap, simple rate limit; loopback-only bind shrinks the population that can flood |
| T7 | **Parser / deserialization attack** | Malformed JSON-RPC, oversized frames, header smuggling | Crash or memory blowup (A4) | Bounded frame size; strict JSON-RPC validation; reject unknown methods cleanly; opaque (non-parsed) token avoids JWT/parser CVEs |
| T8 | **Audit tampering / blind spots** | Attacker actions not logged, or log doctored to hide them | Lose the "who changed what" answer the checklist demands (A3) | Append-only JSONL with success AND error rows; log caller-if-known, tool, spec IDs, result, duration; never log secrets/free-form bodies; consider append-only file perms |
| T9 | **MitM on a non-loopback path** | Plaintext HTTP over a network | Token + graph sniffed in transit | v1 is loopback-only (no wire); "real remote" goes through SSH tunnel (C) or mTLS (D) which provide the encryption — plaintext HTTP is never exposed to a network in v1 |
| T10 | **Confused-deputy via cloud agent** | A hosted agent with the token is itself compromised/prompt-injected | Agent issues legitimate-looking but malicious tool calls | Read-only profile caps blast radius; audit captures every call; write exposure stays behind explicit per-project opt-in; pairs with the AGY-style "draft-not-merge, cross-validate" dispatch discipline |

**Worst case for the substrate, stated plainly**: a networked write-capable
transport whose token leaks lets an attacker rewrite the requirement graph,
which AIDA's own autonomous machinery will then *act on* — turning the substrate
into an execution vector. This is why v1 is read-only and why write-over-HTTP is
the single most important thing held behind operator sign-off.

---

## 5. Smallest-safe-slice (v1)

**Read-only, loopback-only, single-token, single-project, audited.**

1. `aida mcp-serve --transport http --bind 127.0.0.1:<port>` (default transport
   stays `stdio`; HTTP only when explicitly requested — answers Codex's open
   question in favor of "disabled unless `--transport http` is passed").
2. Bind **only** to a loopback address; refuse non-loopback binds in v1 (or gate
   behind a separate, explicit, documented-as-dangerous flag).
3. Require a bearer token via `--token-env`; **refuse to start an HTTP transport
   without a token**.
4. Force the profile to `read-only` for HTTP in v1 (refuse write-capable
   profiles over HTTP); `tools/list` and `tools/call` already enforce this.
5. Reuse the existing tool registry, profile filter, and project scoping
   unchanged — the HTTP layer is purely a frame source/sink in front of the
   existing JSON-RPC dispatch.
6. Append-only audit JSONL at `.aida/audit/mcp-tools.jsonl`: timestamp,
   transport, profile, caller-if-known, tool name, spec IDs parsed from
   args/result where safe, success/error, `duration_ms`. No secrets, no
   free-form bodies.
7. Docs: stdio-vs-HTTP trade-off section; "loopback + SSH tunnel" as the
   blessed remote pattern; update the install-matrix Copilot/Devin/cloud rows to
   say "HTTP token transport (read-only, loopback) is the minimum safe bridge —
   not raw stdio, and not write-capable until a later phase."

What v1 deliberately does NOT do: no write tools over HTTP, no LAN/public bind,
no mTLS, no OAuth/DCR, no multi-project daemon, no per-client token table, no
TLS termination inside AIDA (tunnel handles it).

Suggested phasing after the slice:
- **Phase 2**: coordination-profile writes over HTTP, behind explicit per-project
  opt-in + per-client tokens + the audit log proven in v1.
- **Phase 3**: non-loopback exposure only via mTLS (D); SSH tunnel (C) remains
  the recommended low-ceremony remote path throughout.
- **Spike (separate)**: OAuth 2.1 / DCR for a hosted AIDA MCP — only if a hosted
  offering becomes a real product direction.

---

## 6. What needs operator SECURITY sign-off before implementation

Each item below is a decision the operator must make explicitly. No code lands
until these are answered in writing.

1. **Build trigger.** Confirm a *concrete* remote/cloud-agent need now exists
   (STORY-475 is deferred until "post-stability + concrete need"). If not, this
   stays a sketch.
2. **Default-off confirmation.** Confirm HTTP is opt-in per invocation
   (`--transport http`), never enabled by ambient config, never a default.
3. **Bind policy.** Approve loopback-only for v1 and the explicit rule that
   non-loopback bind is forbidden / hard-gated.
4. **Token storage.** Approve `--token-env` + `0600` `.aida/` token file; confirm
   the token file stays gitignored (no `!.aida/<name>` allow-line) and that the
   path is documented.
5. **Read-only-over-HTTP-only for v1.** Approve that write-capable profiles are
   *not* reachable over HTTP in v1, and that lifting this is a separate
   sign-off (the single highest-risk decision — see T3).
6. **Audit log contents.** Approve exactly which fields are logged and the
   explicit redaction rule (no secrets, no free-form bodies); approve the
   `.aida/audit/mcp-tools.jsonl` location and append-only/retention posture.
7. **Caller identity.** Decide what "caller identity if known" means for v1
   (loopback gives little beyond peer pid/uid) and whether SO_PEERCRED-style
   peer-cred capture (likely via a Unix-domain-socket variant) is in or out.
8. **DoS posture.** Approve connection/request-size/rate caps and the failure
   behavior when exceeded.
9. **Marketplace checklist re-run.** Commit to re-running
   `docs/security/marketplace-publication-checklist.md` (§4 MCP tool exposure,
   §5 auth/secrets, §7 auditability) and producing the publish/no-publish
   verdict block before any package advertises HTTP transport.
10. **Test gates required before merge.** Confirm the minimum test set:
    HTTP initialize/tools/list/call smoke; missing-token rejected; write profile
    refused over HTTP; read-only lists only read tools; audit row on success AND
    error; token value never appears in audit or argv; out-of-project-root path
    rejected. (PR CI also runs `tests/test_mcp_stdio.sh` + fmt + clippy — a new
    HTTP suite must join, and the stdio black-box suite must stay green.)
11. **Threat-model acceptance.** Operator explicitly accepts the §4 threat model
    (or amends it) as the baseline the implementation is measured against.

---

## 7. Related

- STORY-474 (shipped) — MCP tool profiles; the permission model this transport reuses.
- `docs/security/marketplace-publication-checklist.md` — §4/§5/§7 gate any HTTP-transport publication.
- `docs/agents/aida-mcp-install-matrix.md` — Copilot/Devin/cloud rows depend on this transport existing before write tools are safe.
- Codex architecture sketch (STORY-475 comment, 2026-05-26) — the v1 shape this document expands into a full threat model.
- CLAUDE.md "deny-by-default `.aida/` gitignore" (BUG-73) — the token file and audit log must respect it.
- `aida-cli/src/mcp.rs` — `run_mcp_server` (stdio entry), `McpProfile`, `tool_in_profile`, `resolve_mcp_profile` (reuse points).
