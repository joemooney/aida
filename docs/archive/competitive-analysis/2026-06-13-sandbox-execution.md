# Sandboxed / containerized agent execution — competitive analysis + recommendation

- **Date:** 2026-06-13
- **Spec:** SPIKE-61
- **Status:** Research synthesis (feeds the living competitive-analysis set)
- **Method:** 3 parallel web-research agents (competitor landscape ×2, Linux sandbox tech ×1) + direct code-mapping of the AIDA repo. Every competitor/tech claim carries a source URL; the claims least likely to survive re-verification are flagged in [§7](#7-claims-im-least-sure-of--verify-before-relying).

---

## 1. TL;DR — the recommendation

**The gap is real but narrower than it looks, because AIDA already has the seam.** As of STORY-567 (Completed, on main) AIDA has an opt-in **`[agents] contained`** posture that turns on Claude Code's *own* Bash sandbox (bubblewrap on Linux) instead of bypassing permissions. What's missing is (a) an AIDA-controlled **OS boundary around the whole `claude -p` process** (the contained mode only sandboxes Claude's Bash tool, not Edit/Write/MCP/network), and (b) **network-egress allowlisting**, which is the single most differentiated competitor capability.

**Recommended shape** (confirms the advisor's prior, with one correction):

| Profile | Backend | Why |
| --- | --- | --- |
| **Cheap local-dev isolation** | **bubblewrap** (rootless, the engine Claude Code already uses), optionally + Landlock on kernel ≥6.7 | Zero overhead on `cargo build`, no daemon, no root, surgical filesystem scoping to the worktree. |
| **Stronger unattended-drain isolation** | **Firecracker microVM** (true kernel boundary + host-enforced egress allowlist), with **gVisor** as the no-KVM fallback | A drain runs with bypassed permissions, unsupervised — that's the threat model that justifies a real VM boundary. |

**Correction to the prior:** the advisor's prior named "rootless Podman/bubblewrap" for local and "gVisor or microVM" for unattended. Research **overturns Podman as the local first-choice** (rootless networking + bind-mount UID mapping add friction bubblewrap doesn't have, and bubblewrap is already in the stack via Claude Code) and **promotes Firecracker over gVisor for unattended** (gVisor imposes a real, FS-metadata-driven `cargo build` slowdown and lacks the iptables NAT table that clean egress allowlisting wants; a microVM has neither problem). gVisor stays as the fallback when `/dev/kvm` isn't available.

**The egress allowlist is the headline feature, and it is *not* a property of any sandbox** — it's host/proxy-enforced. The directly-reusable template is Anthropic's devcontainer `init-firewall.sh`: default-DROP + an `ipset` allowlist of GitHub meta-API IP ranges, `registry.npmjs.org`, crates.io, and the project's git remote.

**Smallest-valuable-slice first** — see [§5](#5-recommended-approach-for-aida-smallest-slice-first). Slice 1 is ~a day: harden the *existing* contained posture with egress config. The OS-wrapper `[sandbox]` posture is slice 2.

---

## 2. Competitor landscape

### 2.1 Comparison table

| Tool | Isolation mechanism | Local / cloud | Network egress (default) | Filesystem scope | Setup friction |
| --- | --- | --- | --- | --- | --- |
| **Claude Code** | macOS Seatbelt; **Linux bubblewrap + socat + optional narrow seccomp** (Bash tool); devcontainer (Docker) ref impl; web = managed VM | Both | Bash sandbox: **prompt per new domain**; devcontainer: **default-DROP + ipset allowlist** | Write = workdir+TMPDIR; **read = whole machine by default** (creds readable) | Sandbox **off by default**; Linux needs `bubblewrap`+`socat`; falls back unsandboxed if missing |
| **OpenAI Codex CLI** | macOS Seatbelt; **Linux bubblewrap + seccomp by default** (Landlock now *legacy* fallback) | Local | **Off by default**; opt-in `network_access=true` | `read-only` / `workspace-write` / `danger-full-access` | Near-zero — sandbox on by default |
| **OpenAI Codex Cloud** | Containers, 2-phase (setup w/ net+secrets → agent w/ neither) | Cloud | **Deny by default** in agent phase; allowlist + HTTP-method filter | Per-repo container | Low (auto-detects deps) |
| **Cursor (cloud agents)** | "Isolated Ubuntu VMs" — **virt tech undisclosed** | Cloud | Has net; **no documented egress controls** | Per-VM cloned repo | Not zero-config (`.cursor/` setup) |
| **Cursor (local agent)** | OS sandbox: macOS builtin / **Linux Landlock v3 (kernel ≥6.2)** / WSL2 | Local | **Blocked by default**, allowlist via `sandbox.json` | Read all + read/write workspace only | Default-on protection |
| **Devin (Cognition)** | Ephemeral isolated "Devbox" — **VM/container undisclosed** | Cloud-only | **Deny by default** + domain allowlist | Per-session FS | Cloud-only, Snapshot config |
| **OpenHands** | **Docker** (default) / host-process (no isolation) / remote | Both | **Open** (stock Docker bridge); no firewall documented | Volume mounts + CoW overlay | Docker required |
| **SWE-agent** | **Docker via SWE-ReX** / local-none / remote (Modal=gVisor, Fargate=Firecracker) | Both | Docs silent (stock Docker); strong isolation only from *remote* providers | Repo inside chosen image | Docker required |
| **Aider** | **None built-in**; optional Docker image (convenience, not security) | Local | None | Unrestricted working tree | Zero (local); Docker opt-in |
| **Google Jules** | Cloud VM (tech unspecified) | Cloud-only | **Open internet** (no allowlist documented) | Per-task ephemeral VM, ~20GB | Low (connect repo) |
| **GitHub Copilot coding agent** | Ephemeral env **on GitHub Actions** + firewall | Cloud-only | **Default-deny allowlist** (covers OS/registry/CA hosts) — but **only the Bash tool; excludes MCP + setup steps** | Repo; can push only to `copilot/*` | Low (firewall on by default) |
| **Replit Agent** | **Hardened Linux containers + seccomp-bpf → migrating to microVMs** | Cloud-only | Not detailed | Per-user sandbox | Zero (managed) |
| **Sourcegraph Amp** | **None built-in**; command allowlist + policy plugins | Local | None | Workspace-oriented, not enforced | Low; isolation is user's job |
| **Factory.ai (Droids)** | "Sandboxed" — **mechanism undocumented**; persistent (BYOM = full host access by design) | Both | BYOM outbound-only; else undocumented | BYOM = full host FS | Cloud low; BYOM = register machine |

### 2.2 Market pattern — what's table-stakes, what's differentiated, where the gaps are

- **Table-stakes (cloud agents): ephemeral per-task isolation.** Every cloud agent (Devin, Jules, Copilot, Codex Cloud, Cursor, Replit) destroys the environment after the task. AIDA's git-worktree-per-implementer is the *local* analog of this but provides **no OS boundary** — only path separation.
- **The differentiator is network-egress control, and the field is split.** Default-deny + allowlist is shipped by **Codex Cloud, Devin, GitHub Copilot agent, Codex CLI, Cursor local**. It is conspicuously **absent / open** in **Jules, OpenHands, Aider, Amp**. This is the single capability that most cleanly separates "security-serious" from "convenience-first." **AIDA today is in the open/none camp.**
- **Local OS-sandboxing is now a two-horse race: bubblewrap (Anthropic, OpenAI) and Landlock (Cursor).** Both are unprivileged, both scope the filesystem to the workspace. Codex's recent move *away* from Landlock-primary *to* bubblewrap+seccomp is a signal that bubblewrap is the more complete primitive today.
- **Strong isolation (VM/gVisor) is a cloud/remote-only story.** No *local* agent ships a microVM; the strong boundaries (Modal=gVisor, Fargate=Firecracker, Replit→microVM) are all someone-else's-infrastructure. **An opt-in local strong-isolation mode would be genuinely differentiated** — but it's a slice-2+ ambition, not the entry point.
- **Honest framing for positioning:** even the leaders document holes — Claude Code's sandbox is off by default and reads credentials by default; GitHub's firewall doesn't cover MCP servers or setup steps. The bar for "credible sandboxing" is *an opt-in, default-deny egress allowlist + worktree-scoped filesystem*, not perfection. AIDA can clear that bar cheaply.

---

## 3. Linux sandbox technology — tradeoffs

Use case: a headless agent doing git + `cargo` + `npm`. Confidence flags inline; perf/kernel specifics that need primary-source verification are in [§7](#7-claims-im-least-sure-of--verify-before-relying).

| Tech | Isolation strength | Compile perf | Rootless | Egress control | FS scoping | Toolchain gotchas |
| --- | --- | --- | --- | --- | --- | --- |
| **bubblewrap** | Namespaces; **shared kernel**; no seccomp unless you add one | **Native** (no daemon, no interception) | **Yes** (its whole point) | **All-or-nothing** (`--unshare-net` = loopback only); no allowlist | **Best-in-class** precise binds | Must bind toolchain+certs+resolv.conf; Ubuntu 24.04 userns restriction can block |
| **Rootless Podman** | Namespaces+seccomp; no root daemon; shared kernel | Native CPU; **use native `overlay` (kernel ≥5.13)** not fuse-overlayfs | **First-class** | Userspace net (slirp/pasta) — **no host iptables for free** | `--read-only` + 1 rw bind | Bind-mount UID mapping (`:U`/`keep-id`); git "dubious ownership" |
| **Docker (rootful)** | Namespaces+seccomp+LSM; **root daemon = escalation path** | Native + overlayfs | Bolt-on, low adoption | iptables via daemon (mangles host rules) | `--read-only`+bind | Works natively |
| **devcontainers** | = whatever engine backs it | = engine | = engine | **Ships the best default-deny recipe** (`init-firewall.sh`) | = engine | needs Docker |
| **nsjail** | Namespaces + **seccomp (Kafel) by default** + cgroups | Native | Yes (setuid uid-map) | NET ns + MACVLAN/pasta; allowlist still external | ro/rw binds | Too-tight seccomp can SIGSYS the compiler |
| **Raw namespaces+seccomp** | Same ceiling as nsjail | Native | Yes (manual) | Manual | Manual | You're reimplementing nsjail — **not recommended** |
| **Landlock LSM** | **Self-restriction, not a sandbox**; shared kernel; composable | Negligible | **Yes** (designed for it) | **Per-port TCP only, ≥6.7**; no domain/UDP/DNS | Excellent (≥5.13) | Needs recent kernel; pre-6.2 missing truncate/refer can trip builds |
| **gVisor (runsc)** | **Strongest short of a VM** — userspace kernel intercepts syscalls | **CPU native, but real per-syscall + gofer-FS tax** → `cargo build`/`npm i` are exactly the heavy profile | Constrained | Own netstack; **no iptables `nat` table** → egress via proxy/host | OCI ro-root + rw bind | Occasional syscall-compat gaps; benchmark your build |
| **Firecracker microVM** | **True KVM kernel boundary** (separate guest kernel) — strongest here | **Native** inside guest | No (needs `/dev/kvm`) | **Cleanest** — host-side TAP firewall, guest can't tamper | Host FS simply absent in guest | Must build kernel+rootfs once; needs nested-virt or bare metal |
| **systemd-nspawn** | Container-grade, shared kernel | Native; `--ephemeral` CoW | **No** (needs root) | veth + host iptables | `--read-only`+bind | Requires root → fails the cheap-local goal |

### 3.1 Ranked recommendation — profile (a): cheap local-dev isolation

1. **bubblewrap** (+ Landlock ≥6.7 optionally). Zero overhead, no root, no daemon, surgical FS scoping — and it's *already the engine Claude Code's Linux sandbox uses*, so AIDA inherits a battle-tested dependency. Network is all-or-nothing, so split an **online fetch phase** (`cargo fetch`/`npm install` with net) from **offline build/edit phases** (`--unshare-net`), or layer an egress proxy.
2. **Rootless Podman** — only if you specifically want OCI image ergonomics; pay the bind-mount UID-mapping + userspace-networking friction and remember to use native `overlay`.

### 3.2 Ranked recommendation — profile (b): unattended drain, bypassed perms, semi-untrusted

1. **Firecracker microVM.** The drain runs unsupervised with bypassed permissions — a real kernel boundary is proportionate. Worktree lives on the guest rootfs (host FS not present = nothing to escape to); **egress allowlisting is enforced host-side on the TAP** (default-DROP + ipset for GitHub ranges, crates.io, npm, the git remote) and the guest can't edit those rules; **git/cargo/npm behave exactly as native** — no syscall-compat or userns surprises. Cost: build a kernel+toolchain rootfs once, reuse across drains; needs `/dev/kvm`.
2. **gVisor (runsc)** — the fallback when KVM/rootfs-building is impractical. Strong (stronger-than-namespace) isolation with OCI ergonomics, but accept (i) a **real FS-metadata-driven `cargo build` slowdown** and (ii) egress via a **proxy** rather than iptables NAT.

In both profiles, defense-in-depth with Landlock self-restriction + a seccomp profile + the host-side ipset allowlist is cheap insurance.

---

## 4. AIDA integration fit (code-mapped)

All refs verified against the repo at branch `main` / `codex/story-567-contained`.

### 4.1 How the drain launches `claude -p`

The headless launch path funnels through one argv builder:

- `aida-cli/src/session.rs:841` — **`claude_headless_args_with_posture(prompt, session_id, contained)`** is the single source of the headless argv. Default posture injects `--permission-mode bypassPermissions`; **`contained=true` injects `--permission-mode dontAsk` + `claude_contained_flags()`** instead.
- `aida-cli/src/session.rs:767` `spawn_claude_headless(...)` and `:942` `exec_claude_headless(...)` are the spawn/exec wrappers; both set `AIDA_HEADLESS=1` on the child (`session.rs:786`, `:961`).
- Callers: `aida-cli/src/main.rs:4344`, `:91942` (reviewer/implementer phases), `:100388` (`headless_launch_hint`). The orchestrator's headless phases all route here.

### 4.2 The existing "contained" posture (STORY-567 — the seam already exists)

- `aida-cli/src/session.rs:872` **`claude_contained_flags()`** → `:880` **`claude_contained_settings_json()`** emits Claude Code settings: `sandbox.enabled=true`, `sandbox.failIfUnavailable=true`, `sandbox.allowUnsandboxedCommands=false`, `sandbox.autoAllowBashIfSandboxed=true`, plus `permissions.deny = destructive_command_deny_rules()` (`session.rs:901` — `rm -rf /`, `git reset --hard`, `git push --force`, etc.).
- Posture is read from config: `aida-cli/src/main.rs:33354` **`load_agents_bypass()`** and `:33368` **`load_agents_contained()`** read `[agents] bypass` / `[agents] contained` from `~/.aida/agents.toml` then project `.aida/agents.toml` (project overrides home).
- `aida-cli/src/main.rs:33277` enforces **`[agents] bypass` and `[agents] contained` are mutually exclusive** launch postures.
- CLI surface: the `--sandbox` flag on `aida agent new` / `aida queue work` (`aida-cli/src/cli.rs:604`, `:712`, `:3628`).

**What contained does and doesn't cover:** it turns on Claude Code's *in-process* Bash sandbox (bubblewrap on Linux) and a deny-list. It does **not** (a) confine the `claude` process itself or its Edit/Write/MCP tools at the OS level, nor (b) set `sandbox.network` / `allowedDomains` / `allowManagedDomainsOnly`, so **network egress is unrestricted** in headless contained mode (the per-domain prompt has no human to answer it).

### 4.3 Worktree isolation

- Agents run in **sibling git worktrees** created by the operator/launcher — the printed convention is `git worktree add /home/joe/ai/aida-<branch> -b <branch> origin/main` (`aida-cli/src/main.rs:15203`). The agent registry pins each entry to its `worktree_path` (`aida-cli/src/agent_registry.rs:92`, `:341`).
- `aida-core/src/git_ops.rs:179` `create_store_worktree` / `:213` `worktree add` is the **store** worktree (`.aida-store/`), distinct from the code worktree.
- **This is the natural mount scope:** a sandbox boundary should bind the agent's code worktree rw and the store worktree rw, everything else ro/absent.

### 4.4 The bypass posture (STORY-495)

- `aida-cli/src/main.rs:32667` **`tool_bypass_flags(agent_type)`** generates the bypass launch flags; the default posture is **native/bypass** (faithful launcher — Claude prompts unless told otherwise), with `[agents] bypass=true` the explicit opt-in surfaced by `aida init`'s first-machine prompt (TASK-698).

---

## 5. Recommended approach for AIDA (smallest slice first)

The faithful-launcher principle (native default, one explicit opt-in) and the existing `[agents] contained` work make this incremental, not a rewrite.

### Slice 1 — Harden the *existing* contained posture with egress allowlisting (≈1 day)

Extend `claude_contained_settings_json()` (`session.rs:880`) to set Claude Code's network controls for headless runs: `sandbox.network.allowUnixSockets` off, and an **`allowedDomains` / `allowManagedDomainsOnly`** allowlist seeded with the git remote host + `crates.io`/`static.crates.io` + `registry.npmjs.org` + `api.anthropic.com`. Make the allowlist a `[agents]` / `[sandbox]` config key. This closes the headless-egress hole using machinery Claude Code already ships (bubblewrap + its proxy), with zero new dependencies. **Highest value-to-effort ratio** — it converts AIDA from "open egress" to "default-deny allowlist," the headline competitor capability, by config alone.

### Slice 2 — Opt-in `[sandbox]` posture wrapping the whole `claude -p` spawn (local profile)

Add a third, mutually-exclusive posture alongside `bypass`/`contained`: **`[sandbox] backend = "bwrap"`**. When set, `spawn_claude_headless` wraps the `claude` argv in a `bwrap` invocation that binds **only** the code worktree (rw) + store worktree (rw) + toolchain/`~/.cargo`/`~/.npm` + certs/resolv.conf, with everything else ro, and an egress proxy or `--unshare-net`+online-fetch-phase for network. This gives an **AIDA-controlled OS boundary around the entire process** (not just Bash), reusing the worktree as the mount scope. Respects the faithful-launcher rule: native stays the default; this is one explicit opt-in.

### Slice 3 — Strong-isolation backend for unattended drains (later)

`[sandbox] backend = "microvm"` (Firecracker) with `"gvisor"` fallback, gated on `/dev/kvm` availability, for `--no-human` drains. Host-side TAP egress allowlist (reuse the Slice-1 allowlist config). This is the genuinely-differentiated, no-competitor-ships-it-locally capability — but it carries rootfs-build and ops cost, so it waits until Slices 1–2 prove the seam and there's demand. **Park, don't build yet.**

### Reusable across slices

The **egress allowlist** (git remote + crates.io + npmjs + anthropic) is the same data in all three slices and in every competitor that does this well — define it once as config. Template: Anthropic's devcontainer `init-firewall.sh` (default-DROP + ipset).

---

## 6. Where this leaves AIDA vs the field

- After **Slice 1** AIDA matches the *table-stakes-plus* tier (worktree isolation + default-deny egress allowlist) — ahead of Jules/OpenHands/Aider/Amp, on par with the contained-mode story of Claude Code itself.
- After **Slice 2** AIDA has a local OS process boundary — matching Cursor-local / Codex-CLI.
- **Slice 3** would put AIDA in territory no *local* competitor occupies (opt-in local microVM isolation). That's the differentiation ceiling, not the entry point.

---

## 7. Claims I'm least sure of — verify before relying

Flagged so we don't launder drift into the living doc:

1. **Codex Linux mechanism** — research asserts bubblewrap+seccomp is now default with **Landlock demoted to legacy fallback** (corrects the SPIKE brief's "Landlock LSM" premise). Confirmed against `openai/codex` `linux-sandbox/README.md`, but version-gated — re-check at press time. *(Confidence: med-high.)*
2. **Firecracker "~125 ms boot / <5 MiB overhead"** — widely cited from the AWS NSDI'20 paper but **not confirmed from the README**. Verify against the paper/spec before quoting numbers. *(Low.)*
3. **gVisor `cargo build` slowdown magnitude** — direction is certain (CPU native, syscall+gofer-FS tax hits compile-heavy workloads); **no published build-workload number**. Benchmark AIDA's own build before claiming a figure. *(Low on magnitude.)*
4. **Cursor cloud & Devin & Factory virt tech** — undisclosed; do **not** claim Docker/microVM/AWS. Cursor-cloud "Docker on AWS" was an unverifiable search snippet — omitted. *(High that it's undisclosed.)*
5. **Modal=gVisor / Fargate=Firecracker** — these are **provider** properties, attributed to Modal/AWS, NOT to SWE-ReX/SWE-agent. *(High, but attribution matters.)*
6. **Kernel cutoffs** — Landlock v1=5.13 / v4(net)=6.7 / v6=6.12 and rootless native-overlay ≥5.13 are solid; the latest Landlock ABI (v7–v9) mappings are secondary-sourced — check `landlock(7)` for the target kernel. *(High on the load-bearing ones, low on v7–v9.)*
7. **Ubuntu 23.10+/24.04 restricted unprivileged userns** can break bubblewrap/rootless-Podman — real and impactful for Slice 2; confirm the AppArmor-profile workaround for the deployment distro. *(Med.)*
8. **gVisor missing iptables `nat` table** — from a third-party issue; the netstack moves fast, re-confirm against current gVisor networking docs. *(Med.)*
9. **Replit microVM rollout** "in progress April 2026" — may be default by now; **Nix is Replit's package layer, NOT its sandbox** (common conflation). *(High on the Nix correction.)*
10. **Claude Code's "84% prompt reduction"** is Anthropic self-reported, not independently verified. *(Treat as vendor claim.)*

---

## 8. Sources

**Competitors:** Claude Code sandboxing <https://code.claude.com/docs/en/sandboxing>, devcontainer <https://code.claude.com/docs/en/devcontainer>, permission modes <https://code.claude.com/docs/en/permission-modes>, sandbox-runtime <https://github.com/anthropic-experimental/sandbox-runtime>; Codex security/approvals <https://developers.openai.com/codex/agent-approvals-security>, Linux sandbox <https://github.com/openai/codex/blob/main/codex-rs/linux-sandbox/README.md>, config <https://developers.openai.com/codex/config-reference>, cloud env <https://developers.openai.com/codex/cloud/environments> + internet <https://developers.openai.com/codex/cloud/internet-access>; Cursor cloud agents <https://cursor.com/docs/cloud-agent> + terminal/sandbox <https://cursor.com/docs/agent/tools/terminal>; Devin security <https://devin.ai/security>; OpenHands runtime <https://docs.openhands.dev/openhands/usage/architecture/runtime>; SWE-agent architecture <https://swe-agent.com/latest/background/architecture/> + SWE-ReX <https://swe-rex.com/latest/>; Aider docker <https://aider.chat/docs/install/docker.html>; Jules env <https://jules.google/docs/environment/> + FAQ <https://jules.google/docs/faq/>; Copilot agent firewall <https://docs.github.com/en/copilot/customizing-copilot/customizing-or-disabling-the-firewall-for-copilot-coding-agent>; Replit security <https://replit.com/blog/defense-in-depth-how-replit-secures-every-layer-of-the-vibe-coding-stack>; Amp manual <https://ampcode.com/manual>; Factory droid computers <https://factory.ai/news/droid-computers>.

**Linux tech:** Landlock <https://docs.kernel.org/userspace-api/landlock.html>, <https://man7.org/linux/man-pages/man7/landlock.7.html>, <https://landlock.io/news/4/>; gVisor performance <https://gvisor.dev/docs/architecture_guide/performance/> + networking <https://gvisor.dev/docs/user_guide/networking/>; Firecracker <https://github.com/firecracker-microvm/firecracker> + rootfs setup <https://github.com/firecracker-microvm/firecracker/blob/main/docs/rootfs-and-kernel-setup.md>; bubblewrap <https://github.com/containers/bubblewrap> + <https://manpages.debian.org/unstable/bubblewrap/bwrap.1.en.html>; nsjail <https://github.com/google/nsjail>; Podman rootless overlay <https://www.redhat.com/sysadmin/podman-rootless-overlay>; systemd-nspawn <https://man7.org/linux/man-pages/man1/systemd-nspawn.1.html>; Claude Code firewall recipe <https://deepwiki.com/anthropics/claude-code/6.2-network-security-and-firewall>.
