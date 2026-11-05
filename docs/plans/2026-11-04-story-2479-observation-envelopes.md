# Observation envelopes and isolated intake

Status: proposed; requires master sign-off before implementation.
Owner: STORY-2479. trace:STORY-2479 | ai:codex

## Decision requiring sign-off

The current work transport profile is store-only: `transport_profile.rs`
pins its push and pull refspecs to `aida-store`; `transport_executor.rs`
requires an attached store worktree and an enrolled store baseline. An
observation-only work clone must not need a requirement store or its history.
Adding an independent intake transport changes that trust boundary.

Recommend a separately enrolled, clone-local intake policy and pinned remote,
reusing the private deny-pattern validation primitives where possible, but
requiring neither a store baseline nor a pseudonymous author field. Preserve
the existing work profile's store-only guarantees. The alternative is extending
that profile to govern both branches, which changes its advertised contract
and enrollment model. Master should approve the independent intake boundary
and the wire format below before implementation.

## Proposed envelope v1

Exactly six JSON keys: `version` (integer 1), `kind` (literal `observation`),
`category` (`observation`, `question`, or `request`), `title`, `body`, and
`checksum` (64 lowercase hex characters). Reject unknown or duplicate keys,
invalid types, unsupported versions, and trailing non-whitespace input.

Hash UTF-8 compact JSON containing the first five keys in this exact order:
`version`, `kind`, `category`, `title`, `body`. Use serde_json string escaping;
do not normalize Unicode, whitespace, or line endings. SHA-256 excludes the
checksum field. Display `INTENT-` followed by the first 12 digest characters;
use the full digest for all identities, filenames, and comparisons.

Proposed limits: nonempty title, at most 512 UTF-8 bytes; body at most 64 KiB;
serialized envelope at most 128 KiB. Bound input reads before parsing. Run
configured deny patterns over both title and body. Diagnostics identify the
rule without echoing matched text. Validation must explicitly say it does not
certify sanitization. Never infer identity, paths, spec references, or metadata.

## Commands and isolation

Keep `aida intent <SPEC>` comprehension compatible. Add `note`, `validate`,
`export`, `publish`, `pull`, `list`, `process`, and `recover-comment`
subcommands. Dispatch local envelope operations before store discovery,
identity setup, cache refresh, role prompts, or automatic sync. Note reads only
the explicitly named text file and writes JSON to stdout. Export produces a
self-contained JSON attachment that validate accepts on a machine without AIDA
initialization. Treat every imported string as data; never invoke an LLM or
execute envelope content.

Publish requires an explicit `--reviewed` confirmation and an enrolled intake
policy. Use a dedicated worktree, a permanent clone-common intake lock, and an
empty root commit whose tree contains only the reviewed envelope. Never use
the product index or ordinary store sync. Subsequent commits may add immutable
`observations/<digest>.json` files and separate receipts only. Validate the
entire reachable intake history and tree allowlist before transport, including
root ancestry, paths, modes, envelope digests, and receipts. Reject unexpected
content rather than deleting it. An exact duplicate is a no-op; conflicting
bytes refuse. Network operations use only an explicit `aida-intent` refspec,
disable implicit tag following and additional ref pushes, and never force.
Pull is fast-forward only and validates staged incoming history before moving
the accepted local branch. Concurrent/divergent remote writes refuse with
recovery advice, rather than rebasing or merging automatically.

## Explicit processing and recovery

Default listing is pending. `process` explicitly keeps pending, dismisses,
creates a requirement, or comments on an existing one. Proposed receipts record
version, envelope digest, action, and optional destination ID, separately under
`receipts/<digest>.json`. Receipts never enter outbound observation envelopes.
Creating/commenting spans two persistence domains: implementation must use a
durable operation identity to detect a successful spec write after a crash
before receipt persistence, and refuse ambiguous replay instead of duplicating
the write. Human and JSON views distinguish pending and processed.

Recovery takes an explicit local backup ref, object ID, and comment selector.
Extract only the selected comment into a preview envelope on stdout; do not
copy an object, history, or inferred title/identity. Require the same validation
and explicit publication review as handwritten envelopes.

## Verification after approval

Use isolated repositories and local bare remotes. Test tampering, unknown and
duplicate fields, malformed types, size bounds, configured sensitive text,
email export/validate round trips outside any initialized project, duplicate
publication, parallel writers, hostile incoming trees/history, and crash
recovery during processing. Snapshot product/store refs and indexes around note,
validate, export, publish, and pull to prove isolation. Exercise existing
`intent <SPEC>` parsing alongside new subcommands. Run targeted tests and the
repository formatting check before opening an implementation PR.
