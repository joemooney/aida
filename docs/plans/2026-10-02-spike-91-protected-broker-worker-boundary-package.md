# SPIKE-91: Protected broker/worker boundary — backend comparison, adapter and protocol specification, custody and disposable-proof plan

**Status:** proposal package for independent review — **grants, installs, selects and deploys NOTHING**<br>
**Date:** 2026-10-02<br>
**Spec:** SPIKE-91 (`aida show SPIKE-91 --full`)<br>
**Governing design record:** ADR-61 (P-enrolled-host protected launch boundary) with binding
amendments M1–M5 from architecture signoff `01a0e11e-12db-7cd0-a3d1-39df79961a63`<br>
**Siblings:** TASK-1550 (host-wide admission/cutover inventory,
`docs/plans/2026-10-02-task-1550-host-wide-launch-admission-inventory-and-cutover.md`),
BUG-1669 (owning defect, implementation-held per `01a0e125`), BUG-1679 (identity
admission, §6 here), TASK-1549/ADR-61 (design record)

> **Read this first.** This package is the gate-2 input ADR-61 §6 names: a concrete,
> reviewable boundary proposal that must return to Joe for explicit authorization.
> Naming a backend here is not selecting it; writing an installer design here is not
> installing it; describing credentials here migrates none. **No protected boundary is
> deployed**, and nothing below changes any live launch path. Every open approval is in
> §9, mirroring ADR-61's NOT-YET-GRANTED table.
>
> Per proxy decision `01a0e360` this spike ran as **read-only research**: no builds, no
> VM or container starts, no installs, no privileged tests, no credential or config
> changes. Host facts in §1 come from read-only probes (listed there). Vendor facts in
> §3 come from vendor documentation, cited per claim, not from experiment.

## 0. Scope, method, and this spike's own resource budget

Acceptance mapping: §2 backend comparison · §3 per-vendor credential verdicts (the
`01a0e360` condition and TASK-1549 verdict O3) · §4 typed adapters, M2 observation
scoping, M3 request encoding, quotas/replay/revocation · §5 installer/trust-root/
credential custody and migration · §6 pinned executable/dependency identity admission
(O7, BUG-1679) · §7 disposable privileged fixture plan and real-boundary test matrix ·
§8 cost/operability tradeoffs and the runtime resource budget · §9 decisions returning
to Joe.

Spike execution budget (condition 1 of `01a0e360`, honoured): this session ran only
`uname`, `ls`, `stat -f`, `systemctl --version`, `command -v`, `grep /proc/cpuinfo`,
`nproc`, `free`, `df`, `swapon --show`, `id`, `getfacl /dev/kvm`,
`systemd-detect-virt`, `cat /sys/module/kvm_intel/parameters/nested`, and version
flags of already-installed binaries. Zero builds, zero VM/container launches, zero
package installs, zero writes outside this worktree and the AIDA store.

## 1. Host qualification facts (read-only, observed 2026-10-02)

These answer the probe list in TASK-1549 verdict O2 and are inputs to §2. They are
facts, not a claim that the host qualifies; qualification is a §9 decision.

| # | Fact | Observed | Bearing |
|---|---|---|---|
| Q1 | Virtualisation of the host itself | `systemd-detect-virt` → `none` (bare metal) | Real KVM, no nested-virt dependency for production workers |
| Q2 | KVM device | `/dev/kvm` present, `root:kvm` rw, **plus ACL `user:joe:rw-`** | KVM usable. The ACL also means the agent principal (`joe`) can start VMs today — worker isolation must come from the service principal owning the jail/cgroups, not from KVM access control |
| Q3 | CPU virt | `vmx` on all 6 cores; `kvm_intel nested=Y` | Production workers need no nesting; nesting being on enables the §7 disposable fixture to run as a VM-in-VM on this host |
| Q4 | Kernel | `7.0.0-30-generic`, Ubuntu 24.04.5 LTS | **Qualification risk:** Firecracker documents a specific supported host-kernel list (`docs/kernel-policy.md` in the Firecracker repo); a 7.0 generic kernel is outside any LTS row that policy has historically named. Must be checked against the then-current list before selection — a §9 row |
| Q5 | cgroups | cgroup2fs unified (`/sys/fs/cgroup`) | Jailer's documented cgroup-v2 missing-parent behaviour (M4 citation) applies verbatim; supervisor must attest membership positively |
| Q6 | systemd | 255 (Ubuntu 24.04) | `DynamicUser=`, `RuntimeDirectory=`, socket activation, `LoadCredential=` all available for broker/service units |
| Q7 | Installed tooling | qemu-system-x86_64 8.2.2 present; podman 4.9.3; bwrap 0.9.0; **firecracker, jailer, cloud-hypervisor, systemd-nspawn, runsc absent** | Candidate B (§2) needs no new hypervisor install; candidate A needs the firecracker+jailer static binaries installed by the administrator |
| Q8 | Memory | 70 GiB RAM, zram 17.6 GiB + 4 GiB swapfile, systemd-oomd active (killed a whole terminal scope 2026-09-25) | Worker memory must be hard-capped per VM; the fleet envelope in §8 is sized to never trip oomd |
| Q9 | Disk | root filesystem 90% full, **93 GiB free** | Worker rootfs images must be small, shared-base + per-attempt overlay; a multi-image zoo does not fit. §8 budgets ≤ 20 GiB total for the boundary |
| Q10 | Single login | `joe` is the only interactive principal and is in `sudo` (TASK-1550 F-2) | Confirms the approval endpoint cannot be any surface reachable from `joe`'s sessions; §5 custody design takes this as given |
| Q11 | Load discipline | 2-slot machine-wide cargo gate; 6 cores | §8 throughput budget: worker fleet concurrency 2, never co-scheduled with 2 builds at full width |

## 2. Backend comparison (AC: "Firecracker+jailer against at least one viable isolated-worker backend")

Three candidates were compared; two are viable, one is priced and rejected. **This
section recommends; it does not select.** Selection is Joe's (§9 row 1).

### 2.1 Candidate A — Firecracker + jailer microVMs (the signed sketch's candidate)

* **Kernel/hypervisor assumptions.** Bare-metal KVM (Q1–Q3 hold); host kernel on
  Firecracker's then-current supported list — **currently unestablished for 7.0 (Q4)**;
  a pinned guest kernel image (their docs maintain supported guest configs); cgroup v2
  (Q5). VMM is the statically linked `firecracker` binary, spawned by `jailer`, both
  administrator-installed at pinned digests (§5), never agent-built (ADR-61 §2).
* **Principals.** `aida-broker` (system service user; owns admission state, spawns
  nothing itself but asks the supervisor); `aida-sup` (supervisor service user; sole
  owner of jailer invocation rights); per-attempt worker uid/gid allocated from a
  reserved range by the supervisor (jailer's `--uid/--gid`), destroyed with the
  attempt; `root` only for the immutable installer/updater (§5); the **approval
  principal is none of these and no login user** — it authenticates end-to-end from an
  operator-held device to the broker (§5.3). `joe` holds no role in the trust chain.
* **Approval and one-boot worker channels.** Approval: authenticated channel from the
  operator's device to the broker displaying the exact normalised signed request (M3,
  §4.3). Worker: per-VM virtio-vsock plus the jailer-chrooted per-VM Unix socket —
  Firecracker documents this host↔guest transport (`docs/vsock.md`); application-level
  mutual authentication on top is ours (§4.3): the guest receives a one-boot keypair/
  token minted at reservation, bound to attempt ID + boot nonce + channel generation;
  the host side binds to the broker's epoch. Nothing on an abstract or TCP socket.
* **Private endpoint binding.** Each VM's control socket lives under the jail chroot
  (`/srv/jailer/<attempt>/root/`), owned by the attempt uid, mode 0700 directory owned
  by `aida-sup`; no path component traversable by `joe` or any login principal. vsock
  CIDs allocated per attempt by the supervisor and torn down with it.
* **cgroup/jail/FD checks (M4).** The supervisor attests **before Running**: the
  firecracker PID is in the intended cgroup (read back from `/sys/fs/cgroup`, not
  inferred from jailer exit status — the jailer documentation permits a cgroup-v2
  missing-parent configuration to proceed without moving the process, the M4
  citation); memory/cpu/pids limits applied; jail root owned by the attempt uid;
  inherited FDs beyond the expected set closed (`/proc/<pid>/fd` enumeration); vsock
  CID and socket path match the reservation. Any mismatch ⇒ kill, reconcile, deny.
* **Guardian/supervisor failure model.** Broker, supervisor, guardian are separate
  units. Guardian holds a liveness lease over the supervisor and broker; **guardian
  health participates in admission** (no fresh admissions while the guardian is
  unhealthy), and guardian loss causes an independent live component — a systemd
  `OnFailure=`-coupled terminator unit holding its own kill authority over the worker
  slice — to stop admissions and terminate the worker cgroup slice (M4). Kill/hang of
  each of the three, adapter-connection closure on revocation, and startup/cleanup
  races are §7 matrix rows. The proposed 1s heartbeat / 5s lease / 10s termination
  numbers remain **unmeasured proposals** (O4) until the §7 fixture exists.
* **Fail-closed recovery/update.** Service state (registry, grants, epochs, budgets)
  on a broker-owned volume; on restart the broker loads state, bumps the boot epoch,
  and **adopts nothing**: attempts not positively re-attested are terminated and their
  reservations marked reconciling. Updates replace the immutable bundle (§5) and
  refuse to start on signature/digest mismatch. Restore-from-backup restores
  configuration and audit, never active approvals or budgets (ADR-61 rollback row).
  Suspend fence: grants carry `CLOCK_BOOTTIME` deadlines (kernel documents the
  boottime/wall distinction — `docs.kernel.org/core-api/timekeeping.html`) and a
  resume epoch check; the fence's actual behaviour across suspend is a §7 measurement,
  not an asserted property.

### 2.2 Candidate B — QEMU `-M microvm` KVM guests under a dedicated service principal

Same architecture, different VMM. Differences only:

* **Assumptions.** QEMU 8.2.2 is already installed from the Ubuntu archive (Q7) and
  updated by the distro — no bespoke hypervisor install, and no Firecracker
  kernel-policy dependency (Q4 risk disappears; QEMU/KVM supports the running distro
  kernel by construction). Guest kernel still pinned by us.
* **Jail.** No jailer equivalent ships with QEMU; confinement is composed from
  systemd: per-attempt transient unit with `DynamicUser=`, `PrivateUsers=`,
  `ProtectSystem=strict`, `RootDirectory=` chroot, cgroup limits, plus QEMU's own
  `-sandbox on` seccomp mode and `-M microvm` (no PCI, no ACPI, virtio-mmio only).
  vhost-vsock via a supervisor-opened `/dev/vhost-vsock` FD passed in, so the unit
  itself has no device node access.
* **Tradeoff.** Larger VMM attack surface (QEMU device models; mitigated but not
  removed by `-M microvm` + seccomp) against a smaller *operational* surface (distro
  packaging, no out-of-tree kernel-support matrix). M4 attestation is **identical in
  shape** — read back cgroup/uid/FD/socket state before Running — which is why the
  supervisor specification in §4 is backend-neutral.

### 2.3 Rejected for the boundary role — shared-kernel containers (podman/nspawn/bwrap)

Priced because the tooling is present (Q7): rootful podman under a service user would
cost near-zero install and image logistics. Rejected as the *worker* isolation
boundary: a shared host kernel places the entire kernel attack surface inside the
blast radius of a worker that is **defined** to run hostile full-access agent code,
and ADR-61 §1 requires vendor bypass inside the worker to leave the external
perimeter intact. Containers remain acceptable as *packaging* inside a VM guest, and
bwrap remains useful inside guests for defense in depth. Not a §9 option.

### 2.4 Comparison summary and recommendation

| Axis | A: Firecracker+jailer | B: QEMU microvm |
|---|---|---|
| Hypervisor attack surface | minimal (purpose-built VMM) | larger; reduced by `-M microvm` + seccomp |
| Host-kernel support risk on this host | **open (Q4)** — must check their supported list | none beyond distro support |
| Install/update custody | bespoke static binaries, our trust root (§5) | distro-packaged, distro updates; our pinning still required |
| Confinement primitive | jailer (with the documented cgroup-v2 caveat — attest anyway) | systemd transient unit + seccomp (attest anyway) |
| vsock one-boot channel | documented (`docs/vsock.md`) | vhost-vsock, equivalent |
| Boot latency / footprint | ~125 ms class, smallest | sub-second with microvm; slightly larger RSS |
| Fleet fit at 2 workers (§8) | fits | fits |

**Recommendation (not selection):** Firecracker+jailer **primary**, QEMU-microvm
**named fallback**, selected automatically only in the proposal sense: if the Q4
kernel check fails against Firecracker's then-current supported list, the package
returns to Joe recommending B. Both candidates satisfy M4's attest-don't-trust rule
identically, so the supervisor/guardian/adapter design below is invariant under the
choice — which is what makes deferring the selection to Joe cheap.

## 3. Per-vendor minimum-credential verdicts (O3; condition 2 of `01a0e360`)

Design premise that frames all three verdicts: inside the boundary a worker **never
holds a real upstream credential**. Model/forge/artifact traffic terminates at typed
adapters (§4); the guest gets a one-boot opaque token minted at reservation, and the
adapter — which holds the real credential — validates that token per request. So each
vendor CLI needs exactly two documented affordances to run inside a worker: **(a)** a
non-interactive credential input (env var or equivalent) and **(b)** a base-URL
override pointing its API traffic at the adapter endpoint. Each verdict below is
sourced from vendor documentation, as `01a0e360` requires; none was established by
experiment.

### V-CLAUDE — Claude Code: **YES, supportable as documented**

* Non-interactive credential: `ANTHROPIC_API_KEY` (sent as `X-Api-Key`; in `-p`
  non-interactive mode "the key is always used when present"), `ANTHROPIC_AUTH_TOKEN`
  (sent as `Authorization: Bearer` — documented precisely for gateways/proxies that
  authenticate with bearer tokens), `apiKeyHelper` script for rotating short-lived
  tokens (re-run on TTL `CLAUDE_CODE_API_KEY_HELPER_TTL_MS` or HTTP 401), and
  `CLAUDE_CODE_OAUTH_TOKEN` for CI. Source: Claude Code authentication docs,
  <https://code.claude.com/docs/en/authentication> (fetched 2026-10-02).
* Endpoint override: `ANTHROPIC_BASE_URL` (same page, credential-management section).
* Filesystem surface: config/credentials relocatable wholesale via
  `CLAUDE_CONFIG_DIR`; on Linux credentials live under `~/.claude/.credentials.json`
  (mode 0600) — in a worker this is a fresh guest HOME, no host HOME mounted.
* Boundary fit: one-boot bearer token via `ANTHROPIC_AUTH_TOKEN` +
  `ANTHROPIC_BASE_URL=` adapter endpoint. No reusable OAuth, no login HOME, no
  keychain. The adapter speaks the Anthropic Messages API surface.
* Caveats: claude.ai-login-only features (connectors, remote control) are absent in
  this mode — acceptable, those are outside a phase worker's job.

### V-CODEX — Codex CLI: **YES, supportable as documented — with a billing-posture consequence**

* Non-interactive credential: API-key auth is documented alongside ChatGPT sign-in;
  cached credentials live in `auth.json` under `CODEX_HOME` (default `~/.codex`), and
  the documented `cli_auth_credentials_store` options include **`ephemeral`**
  (credentials held only in process memory — exactly right for a one-boot worker).
  Source: Codex authentication docs, <https://developers.openai.com/codex/auth>
  (surfaced via search excerpts 2026-10-02; the page 308-redirects to a host this
  session could not resolve — **re-verify the exact prose at package review**, §9
  row 8).
* Endpoint override: `model_providers.<id>` in `config.toml` under `CODEX_HOME` with
  `base_url` plus `env_key` naming the env var whose value is sent as a bearer token;
  command-backed auth with a `refresh_interval_ms` cadence is documented for rotating
  tokens. Source: Codex configuration docs,
  <https://developers.openai.com/codex/local-config> (same re-verify note).
* Boundary fit: guest `CODEX_HOME` with a provider block pointing at the adapter,
  one-boot token in the `env_key` variable, `cli_auth_credentials_store = "ephemeral"`.
  The adapter speaks the OpenAI-compatible API the provider block expects.
* **Consequence to accept:** the fleet's current Codex use rides ChatGPT-plan OAuth —
  a browser sign-in whose cached `auth.json` is a reusable refresh credential, which
  ADR-61 §1 forbids inside workers ("no reusable upstream credentials"). Inside the
  boundary, Codex runs on API-platform billing through the adapter. That is a cost/
  billing change, listed in §9 row 4 for Joe, not a technical blocker.

### V-AGY — Antigravity CLI: **QUALIFIED YES for model traffic; account-session features unsupported**

* Non-interactive credential: the documented headless path is `GEMINI_API_KEY` plus
  `"modelProvider": "gemini"` in `~/.gemini/antigravity-cli/settings.json`; the CLI
  then "skips the sign-in screen", establishes no account session, and the docs state
  this suits headless/CI runs. Default interactive auth is Google-account OAuth with
  tokens in the OS keyring — a reusable credential plus a keyring dependency, both
  unavailable and forbidden in a worker, so the OAuth path is **refused** inside the
  boundary. Source: Antigravity CLI install/auth docs,
  <https://antigravity.google/docs/cli/install/> (fetched 2026-10-02).
* Endpoint override: `GOOGLE_GEMINI_BASE_URL` (same page) — the adapter terminates
  auth, so the one-boot token rides `GEMINI_API_KEY` against the adapter endpoint and
  never is a real Google key.
* Caveats: (1) features requiring an account session (agent-manager/cloud surfaces)
  are absent in API-key mode — the phase-worker role doesn't need them, but any AIDA
  route that does cannot run Agy inside the boundary until separately designed;
  (2) the vendor docs here document no credential expiry or scoping semantics for
  this mode, so the adapter's own quota/revocation (§4.5) is the only enforcement —
  acceptable because that is already the design, but worth naming; (3) enterprise
  docs reference ADC for headless enterprise auth (search-surfaced, unfetched) —
  irrelevant under adapter-terminated auth, noted for completeness.

**Net O3 answer:** the boundary does **not** ship empty. All three fleet vendors have
documented non-interactive credential injection *and* documented base-URL overrides,
so all three can run inside workers under adapter-terminated auth. The one product
consequence is V-CODEX's billing posture change.

## 4. Typed adapters, observation scoping (M2), and the frozen request encoding (M3)

### 4.1 Adapter inventory (typed; nothing generic)

Three adapter families, each a separate service unit under its own uid, holding its
own upstream credentials via systemd `LoadCredential=`, reachable **only** over the
worker's one-boot channel and the broker's internal socket — never a host TCP port:

* **Model adapters** — `model.anthropic`, `model.openai-compat`, `model.gemini`.
  Pinned upstream endpoints; allowed-model lists per grant; request/response passed
  through with size limits; streaming allowed; tool-use content is data.
* **Forge adapter** — `forge.github` (gitlab mirror later). Typed verbs only:
  `clone_ref`, `fetch_ref`, `push_branch` (to grant-named refs only), `open_pr`,
  `comment`, `read_checks`. **No merge verb exists** — a launch grant never confers
  merge authority (ADR-61 decision 7); merging stays with the operator-held flows.
* **Artifact adapter** — content-addressed put/get of declared inputs/outputs.
  Imports require enrolled-source + approved-lineage authorization, not hash
  possession alone (M3: "an artifact hash establishes content identity, never
  permission").

Absent by construction: generic HTTP proxy, credential helper passthrough, arbitrary
remote URLs, shell adapter, writable host mounts (each would recreate the bypass —
ADR-61 §1).

### 4.2 Repo-scoped observation and attachment (M2)

Every read surface carries the same (repo ID, enrollment generation, attempt)
scoping as writes: status/attach/result APIs answer only for the authenticated
attempt; public proposal/status IPC exposes only deliberately public metadata, and
opaque IDs unlock nothing (possession-of-handle grants nothing). Operator data access
is a separately authenticated admin surface, not a widened agent surface. No generic
FD/socket/terminal bridge between host and guest. Rendered guest output is hostile:
the approval device and admin displays neutralise terminal control sequences before
display. Repo-A-requests-repo-B observation is a mandatory negative row in §7.

### 4.3 Frozen signed request encoding (M3) — to freeze **before any protocol code**

* **Canonical form:** a versioned, length-prefixed, field-tagged binary encoding
  (deterministic CBOR subset or equivalent); no JSON on the signature path; unknown,
  duplicate or ambiguous authority fields ⇒ reject, never ignore.
* **Domain separation:** every signature input starts
  `aida-p1|<purpose>|<schema-version>` with distinct purposes at minimum:
  `approval-request`, `approval-grant`, `controller-delegation`, `worker-boot`,
  `adapter-call`, `audit-record`. Separate key pairs and endpoints per certificate
  purpose; a worker-boot credential can never validate as a delegation.
* **Bound fields (per M3, all inside the signature):** operation + phase; repo ID +
  enrollment generation; base artifact and spec/plan content digests; artifact/input
  policy; broker identity + epoch; single-use nonce; limits/budgets; detach policy;
  channel generation and request ID bound to the authenticated transport, so replay
  across reconnect, boot or endpoint neither repeats effects nor restores quota.
* **Display rule:** the approval device renders exactly the normalised fields the
  broker verifies — same bytes, decoded, no client-side re-summarisation.
* **Required adversarial tests** (§7 rows): parser differentials (two decoders must
  agree or refuse), request substitution, role confusion across purposes, cross-repo
  artifact-hash replay.

### 4.4 One-boot worker channel

At reservation the broker mints the attempt record (attempt ID, boot nonce, channel
generation, budgets) and the supervisor provisions the vsock/socket pair (§2). First
guest message proves possession of the boot token over that channel; the broker binds
the channel generation into every subsequent adapter call (§4.3). Reconnect is
broker-initiated only; a second boot of the same attempt invalidates the first's
channel generation.

### 4.5 Quota, replay, revocation

Budgets (tokens, requests, wall-clock, forge pushes) are transactional server-side
counters charged to the grant tree; exhaustion is a typed refusal. Revocation closes
the grant, its delegations, and **active adapter connections** (an open stream does
not outlive its authority — M4/§7 row). Nonces are single-use per broker epoch;
epoch rotation (restart, suspend-resume, update) invalidates in-flight nonces and
re-requires attestation. Nothing is retroactive: completed external effects stand
(ADR-61 honest-limits).

## 5. Installer, trust root, and credential custody proposal

### 5.1 Installed bundle and trust root

* Offline-generated root key (operator-held, never on this host); signs a release
  manifest (binary digests, guest kernel/rootfs digests, schema versions, policy).
* Immutable install under `/opt/aida-boundary/<version>/` (root-owned, 0755 dirs,
  0644/0755 files), with service state under `/var/lib/aida-boundary/` owned by the
  service users; agents' `$PATH` and `aida dev activate` never reach either.
* The installer verifies the manifest signature against the pinned root, installs
  units, creates service users, and records the install epoch. Updates are the same
  flow; anti-rollback via monotonically increasing manifest serials; a failed
  verification refuses and leaves the previous version active (fail closed).
* An agent-built `target/release/aida` remains a **client only** (ADR-61 §2, H1/F-1):
  it can propose over public IPC and can never launch. The dev-loop story stays
  TASK-1550 F-1's open row for Joe.

### 5.2 Credential custody and migration

* Upstream credentials (Anthropic/OpenAI/Google API keys, forge deploy key) are
  created fresh for the adapters at cutover, delivered via systemd `LoadCredential=`
  from a root-owned 0600 store, and the **pre-existing user-held credentials are
  revoked per the TASK-1550 §3 migration window** — migration means new-issue +
  revoke-old, never copying `~/.claude`/`~/.codex`/keyring material into services.
* Which identities move, in what order, and the revocation moment are Joe's (§9
  row 4; shared with TASK-1550 row 3).

### 5.3 Approval endpoint custody (the F-2 answer)

Host fact Q10: no second principal or device exists today. Proposal: the approval
endpoint is an **operator-held hardware device or secondary machine** (phone app or
dedicated enrolled laptop) speaking mutually-authenticated TLS to the broker's
loopback-forwarded listener, paired once through the §5.1 trust root; its private key
never exists on this host. A FIDO2 token (systemd 255 supports `+FIDO2`) is the
minimal concrete option: approval = device-resident key signs the §4.3
`approval-grant` over the displayed request. What it must never be: any PTY, socket,
browser page or prompt reachable from a login session in which an agent can execute
(BUG-1669 AC2; rejected-alternatives list). Device choice and custody are Joe's (§9
row 2).

## 6. Pinned executable/dependency identity admission (O7 — the BUG-1679 contract)

Per advisor verdict O7 this contract lives here, under this spike's "pinned
executable/dependency identity admission" deliverable; BUG-1679's own AC1 is
superseded by it (recorded on BUG-1679 in comment `01a0e35a-6425`).

* The broker executes only identities present in the signed release manifest (§5.1):
  vendor CLI binaries/versions inside the guest image, the guest kernel/rootfs, and
  the AIDA worker shim — all by digest, measured at image build, re-verified at boot.
* `AIDA_BIN`, `PATH` lookups, client version strings, executable paths and
  `AIDA_AGENT_CMD`-class overrides are **untrusted input everywhere** (ADR-61
  decision 13). A client may *name* a candidate; the service admits only what the
  manifest already pins. Unknown/unadmitted identity ⇒ typed refusal, no fallback.
* Identity admission is necessary but is never itself a launch grant (ADR-61 §10):
  an admitted binary still needs the full §4 authority chain.
* If package review shows admission needs its own installer/update-policy design
  beyond §5.1's manifest, that returns as a separate spec — knowable only now that
  this contract is written (the ADR's stated test).

## 7. Disposable privileged proof plan and real-boundary test matrix (gate 3 input)

### 7.1 Disposable fixture (requires §9 row 3 approval before anything runs)

* **Form:** a disposable Ubuntu 24.04 VM **on this host** using the already-enabled
  nested KVM (Q3) — giving the fixture its own `/dev/kvm` for real jailer/QEMU worker
  boots — or an equivalent rented disposable host if Joe prefers zero local footprint.
  Inside it: real installer run as the fixture's root, distinct `admin`/`agent`/
  approval principals (the agent user deliberately **not** in sudo/kvm), a
  fixture-only enrolled approval device key, fixture-only API credentials with
  spend caps.
* **Budget (condition 1 discipline carried to gate 3):** fixture VM capped at 4 GiB
  RAM / 2 vCPU / 20 GiB sparse disk; runs serially with nothing else heavy; full
  teardown = delete VM image + revoke fixture credentials + destroy fixture device
  key. Never run on the production host principal set.
* **CI stance:** hermetic protocol tests (parser differentials, state machines, M3
  vectors) run in ordinary CI forever. The privileged suite runs on the disposable
  fixture, produces signed evidence artifacts, and **a skipped privileged suite is a
  block, not a pass** (BUG-1669 AC4). Mocks and source review are never labelled
  protection proof.

### 7.2 Real-boundary test matrix (minimum rows; each row = real handlers, per-sink)

| Class | Rows |
|---|---|
| Forgery NEGATIVES (AC4 reversal) | hand-written `DrainState` + matching UUID + live PID ⇒ FAIL; fabricated PTY answering yes ⇒ FAIL; both `IsTerminal` observations forced true ⇒ FAIL; `AIDA_AUTO_COMPLETE(+token)` set ⇒ grants nothing |
| Positive protected path | authenticated approval → protected controller → scoped phase worker → allowed next phase, over the real channels; per inventoried sink (TASK-1550 groups A–G) admission or typed refusal |
| Cross-repo (M2) | repo-A grant used for repo-B launch ⇒ refuse; repo-A caller requesting repo-B transcripts/artifacts/attach/console ⇒ refuse; cross-repo artifact-hash import without lineage ⇒ refuse |
| Stale/copied/replaced clients (M1) | H2 stale binaries and copied worktree clients: may propose (if schema-supported), can never launch; negative tests run **from the untrusted host principal** |
| Protocol (M3) | parser differential, duplicate/unknown authority fields, request substitution, purpose/role confusion, nonce replay across reconnect/boot/endpoint, quota-restore replay |
| Supervision (M4) | cgroup membership attested positively (incl. the jailer missing-parent case); limits/jail/FD/endpoint attestation failures kill+deny; broker, supervisor, guardian kill/hang each; guardian loss ⇒ admissions stop + domains terminate via the independent component; revocation closes active adapter streams; startup/cleanup races |
| Timing (O4) | kill-to-gone latency distribution under load, guardian healthy and killed, against the proposed 10s bound; suspend/resume fence holds (boottime deadlines + epoch) — **measured here or asserted nowhere** |
| Fault/rollback | broker restart mid-attempt (no adoption); update with bad signature (refuse, keep old); backup-restore restores audit/config but no live authority; anti-rollback serial |
| Laundering | forge adapter asked to push to a non-grant ref ⇒ refuse; model adapter asked for a non-granted model ⇒ refuse; adapter credential never observable from guest; no-merge-verb invariant |
| Non-launch surfaces | previews/renderers/dry-runs/status remain non-launch and confer nothing (ADR-61 §4) |

## 8. Cost and operability tradeoffs, and the runtime resource budget

* **Runtime budget (proposal, sized against Q8/Q9/Q11 and the oomd incident):**
  max **2 concurrent workers**, each hard-capped 2 vCPU / 8 GiB memory /
  cgroup-limited IO; broker+supervisor+guardian+adapters together < 0.5 GiB;
  worker base image ≤ 4 GiB shared read-only + ≤ 4 GiB per-attempt overlay, total
  boundary disk ≤ 20 GiB (Q9's 93 GiB free makes a bigger zoo irresponsible).
  Workers and the 2-slot cargo gate are co-scheduled: a worker slot counts against
  interactive-build headroom, and the guardian refuses the second worker while host
  PSI/memory pressure is high. These are **proposals to accept and display**
  (ADR-61 budgets row), not consent.
* **Throughput consequence:** today's fleet pattern is already serial-ish on this
  host (one implementer lane at a time); 2 worker slots preserves that. Cutover
  consequence (which unattended jobs stop) remains TASK-1550 §3's list.
* **Operating cost:** candidate A adds a bespoke binary distribution to maintain
  (trust root + kernel-matrix watching); candidate B rides distro updates but
  carries QEMU's surface. Both add: image rebuilds when vendor CLIs update
  (identity admission makes CLI updates deliberate, §6 — a feature and a chore);
  V-CODEX moves to API-platform billing (§3); an approval device enters daily
  workflow — roughly one touch per full-access wave, amortised by controller
  delegations within scope (ADR-61 §1).
* **What stays cheap:** read-only surfaces, proposals, status, previews — all
  unaffected. Non-full-access (contained/supervised interactive) work is untouched
  until cutover decides otherwise (TASK-1550's manifest).

## 9. Exact decisions and permissions that return to Joe (nothing below is granted)

Refines the ADR-61 NOT-YET-GRANTED rows this spike owns; rows 5/8 of that table stay
with TASK-1550.

1. **Backend selection** — accept A (Firecracker+jailer, with the Q4 kernel check
   against the vendor's supported list as a precondition) or B (QEMU microvm), per
   §2.4's recommendation. Includes exact pinned versions/digests at package time.
2. **Approval endpoint device and custody** — FIDO2 token vs phone/second machine;
   pairing ceremony; loss/replacement policy (§5.3).
3. **Disposable privileged fixture authorization** — approve §7.1 (nested-KVM VM on
   this host, or a rented disposable host), its budget, credentials and teardown.
4. **Credential creation/rotation/migration** — which AIDA-managed identities move
   behind adapters, fresh-issue + revoke-old order (§5.2, with TASK-1550), and
   acceptance of V-CODEX's API-platform billing posture (§3).
5. **Trust-root custody** — where the offline root key lives and who (only Joe)
   signs release manifests (§5.1).
6. **Runtime ceilings and budgets** — accept-and-display the §8 numbers and ADR-61's
   proposed grant lifetimes/supervision bounds; all remain unmeasured until §7 runs.
7. **Identity-admission update policy** — §6's manifest-pinned vendor-CLI update
   cadence (who approves an image rebuild).
8. **Codex documentation re-verification** — the two Codex doc pages in §3 were
   corroborated by search excerpts after a redirect this session could not follow;
   the package reviewer must re-read them at source before Joe signs row 1
   (cheap, minutes).
9. **Dated risk acceptance** (restated, owed to Joe by product, ADR-61 row 9): the
   full BUG-1669 exposure persists for the whole SPIKE→proof→cutover window; this
   package shortens the window but accepts nothing on Joe's behalf.

## 10. Explicit non-claims

* **No protected boundary is deployed**; every full-access route still launches
  behind same-user-forgeable corroboration (ADR-61, TASK-1550 §6). This document
  installs, selects, migrates, measures and proves **nothing**.
* BUG-1669 remains blocked and implementation-held even after SPIKE-91 completes
  (`01a0e125`); gates 2–5 remain OPEN; gate 4 was never granted.
* The §8 numbers and ADR-61's timing bounds are proposals; per O4 they are
  unmeasurable until the §7 fixture exists and **no document may assert them**.
* Vendor verdicts in §3 describe documented affordances, not tested behaviour;
  testing them is a §7 hermetic+fixture concern.
* This spike ran read-only within the `01a0e360` budget; the probe list is §0.

<!-- trace:SPIKE-91 | ai:claude:high | impl:2026-10-02 | by:joe -->
