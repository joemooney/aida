<!-- trace:TASK-0423 -->

# Codex sandbox posture: native-first, no AIDA outer wrapper by default

Date: 2026-06-25
Specs: TASK-0423 (child of EPIC-0419, Claude-to-Codex migration readiness)
Status: Design -- for operator review (DESIGN-ONLY; no production code in this change)
Complexity: Low for the recommendation (a launch-path decision + doctor wiring);
the alternative wrapper is Medium-High

> Generic, cross-vendor framing. The question is whether the AIDA launcher must
> impose its OWN OS sandbox around a Codex run, the way it optionally wraps a
> headless Claude run in bubblewrap, or whether the vendor's native sandbox is
> the right boundary to lean on.

## 1. Recommendation

**Do NOT add an AIDA-managed outer bubblewrap wrapper around Codex by default.
Lean on Codex's native sandbox, and make AIDA *select and verify* the right
native profile instead of imposing a second one.** Add an opt-in escape hatch
(`os_wrap` extended to Codex) only for the narrow case of a host that wants a
single uniform confinement mechanism across vendors. The default posture for a
headless Codex drain is: **`codex exec` under a native non-bypass sandbox mode,
fail-closed if that sandbox can't be established.**

The reasoning: Codex ships a *stronger* native OS sandbox than AIDA's current
`os_wrap`, so wrapping Codex in bwrap would add a weaker, redundant outer layer
(and risk breaking Codex's own confinement, see section 5). The defensible move
is to use AIDA's role as launcher to *choose the safe native profile and verify
it engaged*, not to re-implement confinement AIDA does not own.

## 2. What AIDA wraps today (the asymmetry being evaluated)

From `aida-cli/src/session.rs`:

- `os_wrapped_program_and_args(worktree_root, program, args)` wraps a launch in
  `bwrap` **only when `[contained] os_wrap` (or `AIDA_OS_WRAP`) is enabled** --
  default OFF. The wrap is **write-confinement**: `--ro-bind / /` (whole FS
  readable, read-only), rw-bind the worktree + `.aida-store` + cargo/npm/claude
  caches, fresh `/dev` `/proc` `/tmp`, `--die-with-parent`. Network stays
  shared (never `--unshare-net`). Reads stay broad unless `read_allowlist` is
  set (STORY-617).
- Fail-closed: `bwrap_preflight()` runs a userns self-test; if `bwrap` is
  missing or the kernel blocks unprivileged userns, the launch **errors** rather
  than running unconfined (STORY-612).
- **Scope is Claude-specific.** `claude_program_and_args` passes the literal
  `"claude"`; the function is generic (`os_wrapped_program_and_args` takes an
  arbitrary `program`) but **the only callers pass `claude`**, and only on the
  headless `claude -p` drain paths. The interactive `aida agent new` path is not
  wrapped (TASK-864). Codex, Antigravity: not wrapped at all.

So the launch-path coverage table (TASK-0423 acceptance item 2) is:

| Launch path | Outer AIDA sandbox today |
|---|---|
| Headless Claude drain (`claude -p`) | `os_wrap` (opt-in, default off) |
| Interactive Claude (`aida agent new claude`) | none (TASK-864 deferred) |
| Codex (`codex exec`, via `compete`) | none |
| Antigravity | none |

## 3. Codex's native sandbox (verified 2026-06-24)

Codex's native confinement is materially stronger than AIDA's bwrap
write-confinement, and it is on by default (opt-in enforcement, not opt-out):

- **Modes:** `--sandbox read-only` | `workspace-write` | `danger-full-access` |
  `external-sandbox`. Plus an orthogonal approval policy
  `--ask-for-approval never|auto|...`.
- **Linux mechanism:** Landlock LSM + seccomp-BPF (writes restricted to
  whitelisted dirs via Landlock; network syscalls `connect`/`bind`/`listen`/
  `sendto` blocked via seccomp; `PR_SET_NO_NEW_PRIVS`). Falls back to bubblewrap
  on kernels too old for Landlock.
- **macOS mechanism:** Seatbelt (`sandbox-exec`, default-deny SBPL profile).
  Windows: restricted tokens + ACLs + firewall rules.
- **Process-tree confinement:** YES -- subprocesses the agent spawns inherit the
  policy. (AIDA's bwrap also wraps the whole tree, but Codex does it natively.)
- **Network under `workspace-write`:** egress **blocked by default**; opt-in via
  `[features.network_proxy]` + a domain allowlist. (Contrast: AIDA's `os_wrap`
  leaves network fully shared; egress is a *separate* opt-in via
  `[contained] allowed_hosts` / `managed_domains_only`.)
- **Read posture:** like AIDA's default, reads are broad ("sandboxed processes
  always have full disk read") -- so neither boundary closes read-exfiltration of
  host secrets by default; both need an explicit allowlist for that.
- **Config:** `~/.codex/config.toml` (sandbox defaults, approval policy,
  network proxy); per-invocation `--sandbox` / `--ask-for-approval` overrides.

## 4. Native vs AIDA-wrapper, head to head

| Property | AIDA `os_wrap` (bwrap) | Codex native sandbox |
|---|---|---|
| Default state | off (opt-in) | on (opt-in to *bypass*, not to *enable*) |
| Write confinement | yes (worktree + caches rw, rest ro) | yes (`workspace-write` -> cwd + roots) |
| Network egress default | shared (open) | **blocked** under `workspace-write` |
| Read confinement | broad unless `read_allowlist` | broad by design |
| Process-tree coverage | yes | yes |
| Cross-platform | Linux only | Linux + macOS + Windows |
| Kernel requirement | unprivileged userns (AppArmor friction on Ubuntu 24.04) | Landlock 5.13+ (bwrap fallback) |
| Who maintains it | AIDA | OpenAI (the vendor running the agent) |

The native sandbox wins on default network posture, cross-platform reach, and
maintenance ownership. The only thing AIDA's wrapper offers over native that
native lacks is **a single uniform mechanism across vendors** -- useful to a host
that has already invested in the bwrap AppArmor profile and wants one boundary
to audit. That is a real but niche want, which the opt-in escape hatch covers.

## 5. Why wrapping Codex in bwrap is the wrong default

1. **Redundant + weaker.** bwrap write-confinement is a subset of what Codex's
   Landlock+seccomp already enforces, plus Codex blocks egress by default and
   bwrap does not. Wrapping adds a weaker outer layer around a stronger inner
   one.
2. **Nesting hazard.** Running a Landlock/seccomp/bwrap-based native sandbox
   *inside* an outer bwrap userns nests user namespaces and namespace
   transitions. The AIDA docs already flag this for Claude's own bwrap inside
   `os_wrap` ("nests user namespaces, which some hardened kernels disallow",
   `per-agent-config.md`). Codex's native sandbox setup is more likely to fail
   or behave surprisingly inside an outer namespace than to be hardened by it.
3. **Maintenance.** Codex's sandbox is the vendor's actively-maintained security
   surface across three OSes. AIDA's bwrap path is Linux-only and carries the
   Ubuntu-userns operational tax (the whole AppArmor-profile-vs-sysctl section in
   `claude-bubblewrap-sandbox.md`). Owning a second confinement layer for a
   vendor that ships its own is cost without commensurate benefit.

The honest counter-point: AIDA wraps *Claude* in bwrap precisely because Claude's
*native* sandbox (`[contained] enable`) confines only the Bash tool, leaving
Edit/Write/MCP and the process itself unconfined -- there is a real gap bwrap
fills. **Codex does not have that gap** (its sandbox is OS-level over the whole
process tree), so the motivation that justifies wrapping Claude does not transfer
to Codex.

## 6. The recommended Codex launch posture

For a headless Codex drain (the STORY-683 target), AIDA as launcher should:

1. **Default to a native non-bypass sandbox.** Launch
   `codex exec --sandbox workspace-write --ask-for-approval never` (write
   confined to the worktree, egress blocked-by-default, no approval prompts a
   headless run can't answer). NOT
   `--dangerously-bypass-approvals-and-sandbox` -- that is the current
   `compete.rs` adapter's flag, correct for a throwaway bake-off arm but wrong
   for an unattended drain that may touch a real branch.
2. **Map AIDA's worktree to Codex's write root.** AIDA already runs the drain in
   the spec's worktree (cwd); `workspace-write` confines writes to cwd + configured
   roots. The `.aida-store` sibling worktree must be added to Codex's write roots
   (config.toml `[sandbox] writable_roots` or equivalent) the way `os_wrap`
   rw-binds it -- open question Q2.
3. **Egress allowlist parity.** A drain that builds needs crates.io / github.com /
   npm. Under Codex's default-deny network, AIDA must populate Codex's
   `[features.network_proxy]` domain allowlist with the same set AIDA's
   `[contained] allowed_hosts` would carry -- so the existing AIDA egress config
   becomes the source of truth that AIDA *translates* into Codex's native config
   (open question Q3).
4. **Fail-closed verification.** Mirror the bwrap-preflight philosophy: before an
   unattended Codex drain, verify the requested sandbox mode actually engaged
   (e.g. confirm Landlock available / the mode is not silently downgraded), and
   error rather than run unconfined if a confinement mode was requested and could
   not be established. `aida doctor` should report Codex sandbox capability the
   way it reports `bwrap` availability (`bwrap_availability` is the model to
   follow).

## 7. The opt-in escape hatch (uniform-mechanism hosts)

For the niche "I want one audited confinement boundary across all vendors" host:
extend `os_wrap` to wrap `codex exec` too. The plumbing is mostly already there --
`os_wrapped_program_and_args` is generic over `program`, so passing `"codex"`
instead of `"claude"` is the bulk of it. Two adjustments:

- **Disable Codex's native sandbox when bwrap-wrapped** (`--sandbox
  danger-full-access` *inside* the bwrap boundary) so the two layers don't nest
  namespaces and fight -- the outer bwrap is then the sole boundary. This is the
  explicit, single trade the operator opts into.
- The rw-bind set is the same (worktree + store + caches); the codex auth/cache
  dir (`~/.codex`) replaces `~/.claude` in the rw list.

This stays **off by default** (it inherits `os_wrap`'s default-off + fail-closed
contract) and is documented as "only when you specifically want bwrap as the one
uniform boundary; otherwise Codex-native is stronger and the default."

## 8. What wiring the recommendation implies

- **Launch path:** a Codex-drain launch builder (parallel to
  `session::claude_headless_args`) that emits
  `codex exec --sandbox <mode> --ask-for-approval never <prompt>` with the safe
  default mode -- pure + unit-testable, next to the `compete.rs` adapter table.
  The `compete.rs` bake-off arm keeps its bypass flag (throwaway worktree); the
  *drain* path gets the confined flag set.
- **Config translation:** a small layer that reads AIDA's `[contained]`
  egress/write config and renders the equivalent Codex `config.toml` keys (or a
  `--config`-style override), so an operator configures egress once in AIDA terms
  and AIDA applies it to whichever vendor runs.
- **Doctor:** a `codex_sandbox_availability()` probe + an `aida doctor` row, the
  same shape as `bwrap_availability()` -- reports native sandbox mode availability
  (Landlock present? Seatbelt? proxy configured?) and the fail-closed verdict.
- **Docs:** a `docs/agents/codex-sandbox.md` companion to
  `claude-bubblewrap-sandbox.md`, plus a cross-link in `per-agent-config.md`
  documenting the read/write/egress posture and the verification self-test
  (TASK-0423 acceptance item 5).
- **No change** to the existing Claude `os_wrap` path -- this is additive.

## 9. Open questions for the operator

- **Q1 -- confirm native-first.** Agree that the default for a headless Codex
  drain is native `--sandbox workspace-write --ask-for-approval never` (NOT the
  bypass flag the compete adapter uses, NOT an outer bwrap)? This is the load-
  bearing decision; everything else follows.
- **Q2 -- store worktree as a write root.** Confirm AIDA should add the
  `.aida-store` sibling to Codex's writable roots automatically (the drain writes
  spec YAML there), the way `os_wrap` rw-binds it. If yes, that's a config-write
  AIDA does at launch.
- **Q3 -- egress single-source-of-truth.** Should AIDA's `[contained]
  allowed_hosts` become the canonical egress allowlist that AIDA *translates*
  into Codex's `[features.network_proxy]` domains, or should Codex egress be
  configured directly in `~/.codex/config.toml` and left to the operator? The
  former keeps one mental model; the latter is less code.
- **Q4 -- ship the `os_wrap`-over-Codex escape hatch now, or defer?** It is cheap
  (the generic wrapper already exists) but adds a maintained surface. Defer until
  a host actually asks for uniform confinement, or build it alongside the
  native-first default for completeness?
- **Q5 -- fail-closed strictness.** How hard should the verification fail? Error
  the drain if the requested native mode can't be confirmed engaged (strict,
  matches bwrap-preflight), or warn-and-continue with `--ask-for-approval never`
  treated as acceptance of risk? Recommend strict, matching `os_wrap`.

## 10. Related

- `aida-cli/src/session.rs` -- `os_wrapped_program_and_args`, `bwrap_preflight`,
  `bwrap_availability`, `os_wrap_enabled` (the Claude wrap path + the model for
  the Codex doctor probe).
- `docs/agents/claude-bubblewrap-sandbox.md` -- the `os_wrap` mechanism +
  userns/AppArmor operational tax this design avoids re-incurring for Codex.
- `docs/agents/per-agent-config.md` -- `[agents] bypass` / `[contained]` posture
  knobs and the nesting caveat.
- `aida-cli/src/compete.rs` -- the current `codex exec
  --dangerously-bypass-approvals-and-sandbox` adapter (correct for the bake-off,
  the wrong default for a drain).
- `docs/agents/porting-claude-code-to-codex.md` -- flags `os_wrap` as the
  Claude-launch-specific piece "needing a deliberate Codex wrapper"; this doc is
  that deliberation, and its answer is "native-first, wrapper opt-in."
