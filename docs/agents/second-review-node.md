# A second trusted review node

<!-- trace:STORY-2492 trace:STORY-b6b1278890774ef993d2ebc327862470-418.Docs | ai:codex -->

This workflow lets one Linux host implement a PR and another review it. Both
hosts must use the same code-origin URL spelling, share the git-canonical AIDA
store, and have OpenSSH with `ssh-keygen -Y sign` and `-Y verify`. Publishing is
opt-in. Unconfigured hosts retain their ordinary local review workflow.

## Enroll the hosts

On each host, the operator creates a dedicated ed25519 key outside the project:

```bash
ssh-keygen -t ed25519 -f ~/.ssh/aida-review-node
```

Keep the private key on that host. Supply its public `.pub` file to the operator
performing enrollment. From a direct human terminal in the project, enroll each
host with a distinct stable node ID (letters, digits, hyphens and underscores):

```bash
aida review nodes enroll node-a /path/to/node-a.pub
aida review nodes enroll node-b /path/to/node-b.pub
```

Enrollment commits the public keys into `reviews/v1/nodes.json` in `aida-store`.
Sync the store on both hosts. On each host, from a direct human terminal, configure
its own identity and private key path:

```bash
aida review nodes publisher node-a ~/.ssh/aida-review-node
```

Use `node-b` on the second host. The path and node ID stay in the primary
checkout's ignored `.aida/review-publisher.json`; the private key is never copied
into AIDA. A key requiring a passphrase must already be usable by the host's
OpenSSH signing environment; AIDA does not provide a passphrase prompt. Each
signing or verification process has a ten-second deadline.

## Implement on A, review on B

Use ordinary AIDA authoring sessions so their stable session IDs are recorded.
Opening a PR through AIDA on a configured host publishes signed author-session
evidence bound to the pushed tip of the PR branch (not the HEAD of the checkout
that ran the command). AIDA republishes it whenever it pushes a new head to an
existing PR (ship rework, orchestrator adopt). Author sessions accumulate per
change, so a forge-side update-branch head keeps the earlier exclusions. For a
PR opened or pushed outside AIDA, run this on A from a checkout of the PR head:

```bash
aida review authorship --pr 123 --specs STORY-123
```

The command requires existing author-session evidence for the named specs. It
cannot manufacture independence from missing evidence. If an open or push
cannot publish (for example, the branch tip is not resolvable), the PR still
opens and a warning prints this retry command. Sync B's code and store before starting its ordinary
reviewer session. Its host recorder/launcher collects and validates the review
under the existing local grant or human-review rules, then signs and persists
an immutable publication. The contained reviewer never receives a node key.

After B's store push and A's store pull, A's ordinary ship/train/ready readers
see B's publication. The reviewer session must differ from every author session,
including when a conversation resumes on another node. Process and lease tokens
are node-qualified; stable session tokens remain comparable across hosts.
The PR head must still match exactly. Syncing does not carry approval over a
changed head. Readers select each change separately, at its current head: the
caller's head, or else the tip of the reviewed branch in the local code checkout
(`origin/<branch>`, then the local branch). Publications at other heads are stale.
They neither block nor credit the current head. A spec-keyed read uses the newest
change (highest number) whose publications covered the spec.

An ordinary refusal (`aida review record --finding <TEXT>`, or a collected
reviewer verdict) publishes with its prose findings signed into the receipt.
A refusal may also name filed findings by canonical origin ID; a named ID that
does not resolve leaves the local verdict recorded and the publication pending
(a warning, not a failed record). Fix the reference and retry on B:

```bash
aida review publish --pr 123 --specs STORY-123 --blocking-findings TASK-node-b-12
```

This command revalidates the existing local verdict's authority. It cannot sign
a free-form external verdict. `--blocking-findings` is a comma-separated list of
native origin IDs, not remappable display aliases. Findings themselves must be
committed to the shared store before publication. The same command without
findings retries an approval after a signing or transport failure.

## Evidence, conflicts and revocation

Receipts contain canonical JSON plus a detached OpenSSH `sshsig`, signed in the
`aida-review-receipt` namespace. Verification renders the current committed roster
to a temporary `allowed_signers` file with node IDs as principals. Publications
bind repository, PR/MR, full head, canonical spec UUIDs, decision, blocking finding
origin IDs, reviewer actor, node and stable identity. Author publications bind
the change and exact head to the known author sessions. Neither local grants,
key-possession tokens, nor human-review receipts are exported.

The synced publication takes precedence over the local ledger/file view. A
contradictory local decision is reported. Publications are identified by
`(node, publication UUID)`, never a machine-local ledger sequence. Each publication
explicitly signs the same node's publications it supersedes. Independent concurrent
refusals remain blocking; another node's approval cannot erase them. A malformed,
unsupported or unverifiable relevant publication refuses credit instead of
falling back to a local approval.

To revoke a publishing node, the operator runs:

```bash
aida review nodes revoke node-b
```

Commit/push success and consumer store sync are necessary for other hosts to see
revocation. Once the roster removal is visible, the node's receipts cannot grant
new approval credit. Already merged PRs are not unmerged. A change whose only
publications came from revoked nodes reads as having no synced review (a report
says so), so a fresh review on an enrolled node or locally answers for it. A
receipt covers the change's bound spec set: the specs its authorship was first
published for, whichever spec the recorder named. To replace a key, revoke
then enroll the node with the new public key; old signatures will no longer verify.

Store writes use the ordinary store writer lock and exact-ref pushes. Immutable
publication pushes retry on concurrent store advancement without resetting other
writers; the rebase runs inside the store writer lock. A failed push leaves
local evidence pending; retry with store sync and `review publish`. A roster
push that races another operator leaves the edit committed locally. Run
`aida sync`, then rerun the same `review nodes enroll|revoke` command. The rerun is
idempotent: it only pushes when the roster already holds the edit. If the sync
reports a roster conflict, drop the local roster commit and reapply. Keep operator roster
edits serialized. Offline local evidence gives no credit to a host that has not
received it.

Readers inspect committed Git blobs rather than trusting working-tree files.
Evidence is stored per change under `reviews/v1/changes/<CHANGE>/publications/`
and `.../authorship/`. Unsigned routing markers under
`reviews/v1/specs/<spec key>/<CHANGE>` only route spec-keyed reads; the receipt
must still verify. Version 1 bounds each command's output to 1 MiB, each change's
evidence listing to 256 entries and its aggregate receipt bytes to 8 MiB. An
exhausted budget or a malformed blob refuses that change only. Other changes,
and specs reviewed only locally, are unaffected. When a change's publications
name several heads and a reader that has no head of its own cannot resolve the
branch, those publications are reported and ignored there; head-bound gates
(merge, train) still apply the publication at their head.

## Run distributed watchers

Configure the single authority in the reviewed default-branch configuration;
a branch-local edit cannot redirect admission or credentials:

```toml
[review]
admission_hub = "https://github.com/joemooney/aida.git"
admission_ref = "refs/aida/review-admission"
```

The repository ships with these settings disabled. Enroll and configure every
watcher host first, upgrade every watcher to the admission-aware binary, and stop
older watchers before enabling this configuration in a separate reviewed change.
A host without an enrolled publisher refuses dispatch when the hub is enabled;
it never silently falls back to local-only admission. Old binaries cannot participate in this
protocol or read its extended local launch receipts. Every publishing node must
use the same exact URL and ref. The watcher repository must match that GitHub
authority; another project cannot share its PR-number admission keys. GitHub is the sole
admission authority. GitLab and the hourly reconcile bridge distribute evidence
and never admit. A GitLab MR or a GitLab admission URL refuses. There is no
failover: `admission_hub_unreachable` names the configured authority and no
reviewer starts. A configured publisher with missing admission settings refuses;
a host with neither publisher nor admission settings retains local-only operation.

Run `aida review watch --once` or the ordinary continuous watcher on each host.
Each discovery pass first fast-forwards the clean canonical store through its
existing evidence gate; divergence, dirty state and sync errors refuse that pass.
The watcher then queries open forge changes and reads verified synced publications,
even when there is no local verdict directory. The local two-review ceiling still
applies. Before spawning a dispatcher it signs a Reserved transition and pushes
with an explicit expected-tip lease to the configured ref. It requires a definite
successful push and confirms the accepted transition. Unrelated store commits
do not refuse transitions: short store locks refresh publisher, roster and
relevant verdict evidence before publication; changed evidence rebuilds within
the same retry budget. Network I/O holds no store lock. A losing CAS reloads and
reconstructs against the winner, for at most three attempts, preserving other
changes and tree entries. It never rebases a losing claim into an accepted one.

Each bounded version-1 `changes/PR-N.json` blob contains one signed causal chain.
There is no global change-count or aggregate-blob budget. Legacy `admission.json`
is read only as a fallback for changes without a shard; the first write migrates
that change into its own blob and retains the old table as historical evidence.
Upgrade all admission writers together: old writers cannot see the shards. Each transition binds the repository/forge, hub/ref, GitHub PR, exact head,
unpredictable UUID token, owner node/session, transition ID and predecessor.
Signatures use the enrolled node key and the slice-1 OpenSSH namespace. The
process-bound dispatcher consumes Reserved → Running exactly once after its
head/overlap preflight and before queue work. An observed worker exit permits
Running → Terminal only when every recorded reviewer/worker has stopped and
none is unrecorded. Proven preflight or dispatcher-spawn failure permits
Reserved → Cancelled. Terminal heads are not automatically reviewed again;
Cancelled admits a fresh token. An active older head fences newer heads.

A failed or lost push response, crash, uncertain child spawn, unknown process
identity, revoked signing node or failed terminal write retains capacity safely.
Clock age, reboot and remote PIDs never release a distributed claim. Local
`--release` refuses while the hub has an active claim or an unverifiable chain.

### Recover a stopped or revoked claim

Stop admission watchers on **every node**. Independently verify that every
reviewer and queue-work child for the affected PR has stopped, including children
on rebooted or disconnected hosts. Recovery does not terminate processes or
infer safety from age. Inspect the hub tip and the affected chain:

```bash
git ls-remote https://github.com/joemooney/aida.git refs/aida/review-admission
```

From a direct human terminal on a still-enrolled publisher, use the exact SHA
just inspected (replace `HUB_SHA` and the PR number):

```bash
aida review nodes admission-reconcile PR-123 --expected-tip HUB_SHA \
  --all-reviewers-stopped --reason "owner rebooted; all reviewers verified stopped"
```

This command signs an immutable recovery attestation under `recoveries/<uuid>.json`
and CAS-pushes a new empty anchor for that PR only. It does not require the old
owner's signature to remain valid, so it can recover revoked owners too. The old
chain remains reachable in Git history. Old tokens cannot transition into the
new anchor. A moved tip, missing stopped-process attestation, unenrolled recovery
publisher or uncertain push refuses; inspect the hub again before retrying.
The command never retries a raced recovery against a different tip.

Consumers verify the current shard's signed chain and the recovery anchor's
signature and scope. They do not compare the shard with ancestor commits or
check `predecessor_tip` against Git history. Preserving or resetting chains
therefore relies on trusted writers: a buggy trusted writer could truncate a
shard or replay an old signed anchor without detection by these readers.
<!-- trace:TASK-3196 | ai:codex -->

On each node, run `aida review watch --release PR-123` to clear stopped local
attempts. This now checks that the hub no longer has an active claim, and still
refuses a locally live process. Restart admission only after all nodes finish
cleanup. A fresh review uses a new token; ordinary synced verdict checks still
prevent double-review of a completed head. The same operation can compact a
closed PR's Terminal/Cancelled chain or an unusually long open PR's chain, after
the same stopped-admission checks. Revocation does not require retaining a node
in the roster: re-anchor its affected chains from an enrolled operator host.

A losing node caches a remote hold without using a local reviewer slot; it skips
repeated hub probes for that head until a head change or explicit recovery.
Admission update network commands have ten-second per-command deadlines and run
outside the canonical store writer lock. Only the local evidence snapshot is
read under that lock.

The custom GitHub ref uses client-side CAS, not GitHub ruleset enforcement.
Only cooperating AIDA writers may update it: no bridge, mirror or operator may
force-push, delete or rewind it during admission. Every shard is bounded to
1 MiB and 2048 transitions; exhaustion affects only its own PR and can be
recovered with the stopped-admission command above. Grants and human receipts
remain local; this protocol grants no remote role authority.

Untrusted work-node envelopes are outside this workflow.
