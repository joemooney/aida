# Registry and mail identity

Agent registry runtime rows now store `linux:<boot-id>:<ticks>` in `pid_start`
with `pid_namespace`, captured together through `ProcIdentity`. These local,
unreplicated runtime files supply local provenance; they are not remote-node
observations. Launcher/hook merges, heartbeat ownership and wake transitions
compare the entire tuple. Foreign rows cannot be displaced by a colliding local
PID. Missing boot/namespace or legacy timestamp/raw-tick records remain unknown
for retention and cannot satisfy strict mail-wake or ancestry identity checks.
No passive inspection upgrades an old PID. A locally captured registration
upgrades a legacy raw-tick row in place only when PID and ticks match and its
namespace is matching or absent, preserving launcher metadata; failed coordinate capture asks for
a host retry. Existing listing heartbeat windows remain display policy.

Strict watched-mail and backlog readers require the recorded full local identity
to be positively Alive. Wake passes check that the verified audience still
belongs to the snapshot's process, thread, handle and role. They check the row
generation under short registry namespace lock holds before and after submission.
The allocator lock is released during snapshot and external queue I/O; acceptance
remains ledgered if the generation changes during submission. Explicit role binding
captures and revalidates through the shared observer around existing same-user,
harness/shell and terminal-ancestry eligibility checks. Observation does not
supply cooperative non-subreaper holder admission or replace grant checks.

The registry wire ID is prefixed `proc-identity-v1:`; updated readers strip
it, keeping in-memory IDs, filenames and reservations stable. Older strict
mail readers reject its filename mismatch even on their session-ID selection
path. The start scheme is also boot-qualified, so older lenient process readers
cannot compare it. Unsupported future registry ID versions remain unknown for retention and
refuse strict mail binding. Restart old mail-wake services with
the updated binary before relying on new rows. Registry views and new lease
creation explicitly project raw ticks for their existing display/signal/lease
adapters; the remaining registry/GC and signal inventory stays in S5. Grant
binding tokens keep their existing namespace-qualified wire format, so existing
presenter/review readers remain compatible. No grant authority model changes.
<!-- trace:TASK-3366 | ai:codex -->


Registry and mail wake observe liveness only; neither admits holders or signals
processes. Subreaper adoption does not change their policy: an adopted live process
remains alive, and an exited retained child remains dead. Cooperative holder
admission stays with the authority consumers.

Private inbox binding has a separate namespace visibility policy: a matching
native session ID and a heartbeat at most five minutes old may retain binding
when the full process identity observer reports NamespaceMismatch. Known-dead,
missing identity and other unknown verdicts refuse; CLAUDE_PID alone does not
authorize this fallback. Strict host-side watched-mail remains Alive-only.
