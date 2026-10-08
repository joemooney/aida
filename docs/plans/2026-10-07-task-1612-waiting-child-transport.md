# Waiting-child transport implementation

Owns TASK-1612, bounded by BUG-1808 launch addendum and independent launch-v2
signoff (owner verdict 01a119b5-ea32-7ed0-b5e9-11858700f78b).

1. Add core Linux transport: immutable independently copied ELF images,
   opened-object policy, explicit ephemeral description, owned process identity,
   private credential-bearing seqpacket channel, bounded cancellation.
2. Enter bootstrap before normal CLI initialization. Production can only become
   READY, refuse, time out, or cancel. No production release decoder/constructor,
   publisher, lifecycle schema, grant, occupancy or vendor caller is added.
3. Exercise the actual bootstrap/syscalls with compiled fake leaves, fake HOME,
   temporary roots and owned subprocesses. Test-only release is cfg(test).
   Check replacement, FD closure, credentials, frames, deadline, supervisor and
   creating-thread death, pre-prctl window and orphan reaping.
4. Run targeted core/process/internal CLI regressions, normal and no-default
   checks/builds, workspace tests, formatting and diff checks. Save bounded
   foreground logs using the normal machine Cargo slots.
5. Document limits; commit/push the branch and provide exact-head evidence to
   the advisor for a different reviewer. No PR, merge, installation or parent
   completion. Git-only callback credentials remain a distinct future type;
   this transport introduces no publication format or authority decision.

## Reusable helpers

Existing core liveness probes are observational and may return unknown; they
cannot own cancellation. Existing CLI process_retry respawns Commands; it is
not suitable for same-process descriptor-exec retries. internal_cmd runs too
late for bootstrap, so the entry seam must precede main_entry initialization.

## Validation limits

This does not implement the production launch witnesses, installed vendors,
Node translation, panes, containment, recovery publisher or callback domain.
Trusted system loader/libraries are an explicit entry-image boundary, not the
future Git callback closed-runtime proof. Missing ACK or death is not evidence
that released work never ran.

## Reproduction notes

Run the transport witnesses with `cargo test -p aida-core launch_transport
--lib -- --test-threads=1`, and the normal-build boundary with `cargo test -p
aida-cli --test waiting_child_bootstrap`. The Linux x86_64 GNU fixtures require
`cc`, `strip`, procfs and an isolated user/mount namespace for the noexec-mount
witness. The three ignored worker functions are subprocess entrypoints invoked
by their owning tests; they are not omitted acceptance cases.

Use a fake HOME, an explicit worktree-private CARGO_TARGET_DIR and normal Cargo
slots. Remove ambient coordinating AIDA_BIN/session variables from test child
environments. Guard every ambient vendor name, including `agy` and
`antigravity`; version probes must never reach installed vendors. Keep canonical
coordination on `/home/joe/ai/aida/target/agent/aida` outside those environments.
Fake HOME alone is insufficient for the existing full suite: role-activity
recording discovers the test cwd and can follow the sibling session symlink
into main. Use a byte-matched disposable source export with independent Git
metadata and no live store/session symlinks for the full suite. A read-only
user-namespace attempt protects live files but refuses unmapped system-loader
ownership and prevents existing tests from creating source-local fixtures;
neither failure should be hidden by weakening the transport's host policy.
Preserve any contamination evidence; do not restore shared files over
concurrent activity.
Interrupted runs are not passes. Recovery records must distinguish owned-process
exit observation from reaping by a parent/adopter, and preserve earlier logs.

Kernel references: [executable memfds](https://docs.kernel.org/userspace-api/mfd_noexec.html),
[descriptor exec](https://man7.org/linux/man-pages/man2/execveat.2.html), and
[creating-thread death semantics](https://man7.org/linux/man-pages/man2/PR_SET_PDEATHSIG.2const.html).
These document the primitives; the fixture results establish only the tested
host behavior, not installed-vendor or production integration readiness.
