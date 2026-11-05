# A second trusted review node

<!-- trace:STORY-2492 | ai:codex -->

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

Refusals that contain prose findings need filed findings with canonical origin
IDs. The recorder keeps its local verdict and reports the publication as
pending (a warning, not a failed record) when those references are missing.
Supply them and retry publication on B:

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
new approval credit. Already merged PRs are not unmerged. To replace a key, revoke
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
and specs reviewed only locally, are unaffected.

Watcher reservations remain machine-local in slice 1. Do not run competing
watchers against the same PR heads yet; reservation synchronization is slice 2.
Untrusted work-node envelopes are outside this workflow.
