# STORY-612 — Sandbox slice 2: bwrap OS-boundary around the headless `claude -p` process

- **Date:** 2026-06-13
- **Specs:** STORY-612 (parent SPIKE-61; slice 1 = STORY-605). Follow-ups: STORY-615, STORY-616 (parked), STORY-617.
- **Status:** Implemented; PR open for advisor review (no self-merge). Merge also gated on the CI live-confinement test.
- **Complexity:** Medium (new OS-sandbox posture, security-sensitive, single-file core).

## 1. Approach

Wrap the **whole** headless `claude -p` process in bubblewrap when the opt-in
`[contained] os_wrap = true` is set, giving an OS boundary around Edit/Write/MCP
— not just Claude's Bash tool (which `contained` already sandboxes).

Model = **write-confinement** (operator decision 2026-06-13):

```
bwrap --ro-bind / /            # whole FS readable, READ-ONLY
      --dev /dev --proc /proc --tmpfs /tmp
      --bind   <worktree>      # rw: code worktree (always present)
      --bind-try <store> <~/.cargo> <~/.npm> <~/.claude> <~/.claude.json>   # rw caches/auth
      --chdir <worktree> --die-with-parent
      claude <headless args…>
```

Network stays **shared** (never `--unshare-net` — `claude` needs the Anthropic
API). Egress remains bounded by the slice-1 `[contained] allowed_hosts`
allowlist, not by this FS confinement.

## 2. Decisions

- **Config under `[contained]`, key `os_wrap`** — keeps the whole posture in one
  block alongside slice-1 `allowed_hosts`; avoids the `[sandbox]` name (taken by
  the `aida sandbox` throwaway-store command).
- **Write-confinement, not strict bind-allowlist** — reads stay broad (matches
  Claude Code's own posture; low breakage). Strict read-confinement = STORY-617.
- **Fail-closed** — if `bwrap` is missing OR a userns self-test fails, error with
  remediation; never run unconfined. Asking for an OS boundary and silently not
  getting one is the worst outcome.
- **Headless paths only** — interactive sessions (human present) are not wrapped.
- **Scope trimmed to the wrapper** — managed-settings default-deny (STORY-615) and
  the Firecracker microVM (STORY-616, *parked* per research) split out.

## 3. Files

- `aida-cli/src/session.rs` — core. New: `os_wrap_enabled`, `bwrap_confinement_args`
  (pure), `claude_program_and_args`, `bwrap_preflight`, `which_on_path`,
  `headless_worktree_root`. Wired into `spawn_claude_headless`,
  `exec_claude_headless`, `spawn_claude_headless_resume`.
- `docs/agents/per-agent-config.md` — `[contained] os_wrap` section + prerequisite.

## 4. Tests

- `bwrap_confinement_is_ro_root_rw_worktree_shared_net` — flag shape: ro root,
  rw worktree, chdir, `--die-with-parent`, and **never** `--unshare-net`.
- `os_wrap_off_leaves_launch_unwrapped` — unchanged-posture invariant (off → bare `claude`).
- `bwrap_write_confinement_live_or_fail_closed` — **the live gate.** On a
  userns-capable host (CI runners) spawns the exact bwrap flags and proves
  write-inside-OK / write-outside-blocked live (+ `claude --version` if present);
  on a userns-restricted host asserts the fail-closed remediation error.

## 5. Verification

- `cargo fmt -p aida-cli -- --check`; `cargo test -p aida-cli` (2458 pass);
  `cargo clippy -p aida-cli` (no findings in new code).
- Live `claude -p`-under-bwrap could **not** run on the dev host: AppArmor
  `apparmor_restrict_unprivileged_userns=1` blocks the uid-map. Operator chose to
  proceed without a host-security toggle; the CI live-confinement test is the
  automated gate, and merge is blocked on it.

## 6. Followups

- STORY-615 — headless default-deny egress via managed-settings `allowManagedDomainsOnly`.
- STORY-616 — Firecracker microVM strong-isolation (slice 3, **parked**; gVisor fallback).
- STORY-617 — strict read-confinement hardening (default-absent FS bind-allowlist).

## 7. Related

- SPIKE-61 research: `docs/competitive-analysis/2026-06-13-sandbox-execution.md` (§5 slice 2, §7.7 userns caveat).
- Slice 1: STORY-605 (`[contained] allowed_hosts`).
