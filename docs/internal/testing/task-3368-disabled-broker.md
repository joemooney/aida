# TASK-3368 disabled broker transport

<!-- trace:TASK-3368 | ai:codex -->

Implementation slice 1 follows the advisor's 2026-11-10 ruling and the signed
[D1–D10 sketch](../../plans/2026-11-07-task-3368-proc-identity-broker.md).
**The manual rollout gate is pending.** This slice must publish with
`(Refs TASK-3368)` and does not complete the task or clear a hold/punt.

The Linux-only private `aida-cli-lib::proc_broker` module stages a host service.
There is no command registration, systemd unit, startup hook, consumer adapter,
configuration enable switch, or sandbox socket route. Its `run` entry refuses
unconditionally. Only module tests invoke the private listener. Nothing changes
existing ownership, role, mail, holder admission, or direct observation behavior.

The listener uses AF_UNIX SOCK_SEQPACKET. Startup exercises SO_PEERPIDFD,
SO_PASSPIDFD and an actual SCM_PIDFD/SCM_CREDENTIALS message before binding.
Connection pidfds are feature probes only; every packet is authenticated from
its own kernel pidfd. The pidfd's procfs PID must agree with kernel credentials,
its namespace evidence must be host-local, and its live pin and ProcIdentity
must remain unchanged around validation. There is no numeric-PID pin fallback.
Unexpected rights, missing credentials, truncated control data and oversize
packets refuse, closing all received descriptors through RAII.

Runtime directories must already exist, be owned by the service uid, and have
mode 0700. Directory components are opened without following symlinks; writable
ancestors refuse except root-owned sticky directories above the private runtime.
An exclusive, nonblocking lock is retained for the service lifetime. Socket
binding and validation use the pinned directory; endpoint mode is 0600. Runtime,
lock and socket replacements refuse. Cleanup removes only the original socket.
An existing endpoint is never removed at startup, including after an unclean
exit: the host must inspect and recover it manually while the service is stopped.

Dispatch is serial (one active request, below the sketch's ceiling of sixteen),
with a five-second total accept/request/response deadline and a 64 KiB frame
limit. The closed version-1 request schema includes nonce, project/store,
self-observation operation, subject tuple, holder generation and optional grant
reference. Scope/tuple/schema refusals are structured and reveal no other
project's details. Correct self requests also return `disabled`.

These responses are **refusals, not observation receipts**. No Alive/Dead
verdict, handle or authorization is issued. Grant references are parsed but
never accepted as authority. Nonce-state tests cover bounded replay refusal and
session closure rather than eviction; the staged listener closes after one
frame. Persistent sessions, handle issuance, incarnation/sequence-bound
observation receipts, monotonic expiry, grant/parent revalidation and revocation
serialization remain required before a later observation rollout. No persistent
schema or existing grant policy changes in this slice.

Run targeted tests through the native lane runner:

```sh
aida lane job run -- cargo test --profile agent -p aida-cli-lib --lib proc_broker:: -- --nocapture
# If exit 75: aida lane job wait <job-id>
```

Protocol tests cover strict/duplicate fields, unsupported versions/operations,
invalid tuples, foreign coordinates, forged subject, project/store mismatch,
replay capacity, and unconditional disabled behavior. Kernel tests cover every
packet's credentials, the actual sender over an inherited connection, sender
exit, credential cross-checks, absent credentials, unexpected SCM_RIGHTS,
truncated ancillary data, frame limits, timeout recovery, exclusive locks,
restart and unsafe/replaced runtime endpoints. Negative fixtures include positive
controls. The exact-test subprocess fixture is ignored in ordinary runs and is
executed by its parent test with a bounded watchdog.

Unsupported required kernel options skip real-kernel checks with an explicit
`SKIP real-kernel SCM_PIDFD test` reason. Authentication/procfs failures on a
supporting kernel fail tests. Skips never count as host acceptance. Before any
consumer is enabled, the advisor must review the remaining sketch conformance,
including namespace/masked-procfs fixtures, grant/handle revocation and expiry,
saturation, response binding, and consumer-specific failure/locking behavior.
This synthetic transport evidence grants no containment or sandbox admission.
