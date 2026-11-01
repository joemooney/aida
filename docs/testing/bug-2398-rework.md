# Codex child-context rework evidence

<!-- trace:BUG-2398 | ai:codex -->

The 2026-11-01 advisor ruling and final reviewer13 bundle at
`f05e1b9e6a22c508f60c614a6848423367165465` govern this rework on PR #3207.
The preliminary exec-companion finding was withdrawn. The queue-work caller's
companion refusal and existing companion/judge restrictions remain intact.

The adapter now compares the effective tool environment with a digest of the
explicit intended child's fixed nonsecret whitelist, before the existing grant
validation. The selector read stays in `seat_authority::current_grant_id`;
subject, roster, process/key possession, delegation, expiry and revocation still
use `grant_check`. This checksum provides transport integrity, no authority.
It does not change ADR-66's deliberate same-uid replay residual. Only fixed
cause labels leave the unlinked probe capture, including when Codex reports an
invalid custom config containing private values.

Real installed-Codex fixtures cover later exact-key, set-table and full-policy
conflicts, each with and without a profile, plus profile-only and unrelated
custom configuration controls. A second independently valid same-seat child
is accepted under its own context and refused when substituted for the
intended child. Empty-baseline launches also refuse later policy replacements
that could unmask another selector. Existing user config and KEEP_ME checks,
companion preservation, actual recorder matrices and invalid-grant refusals
remain in the named native and full-access fixtures.

## Exact reviewed CI failure inventory

Run `41301174931`, retained in `/tmp/aida-review3207-ci-failures.log`, contains
21 unique failed tests. All are introduced regressions addressed here:

| Count | Tests | Correction |
|---:|---|---|
| 1 | `only_seat_authority_names_or_reads_the_grant_env_var` | Use `seat_authority::GRANT_ENV`, without exemptions. |
| 1 | `every_gate_module_is_a_dispatch_module_a_kernel_or_an_orchestrator` | Inventory `review_approval.rs` as a protected decision kernel. The whole gate directory remains on the floor. |
| 1 | `no_cli_source_outside_the_resolver_uses_raw_current_executable_lookup` | Use the existing executable resolver for the sibling matching CLI. |
| 1 | `task_2143_no_new_raw_git_where_a_lease_tree_is_read` | Tighten `pr_cmd.rs` from 48 to 47 after the relocation. |
| 1 | `bug_2398_dispatch_flow_ungranted_artifact_requires_fresh_authorized_review` | Invoke the built matching CLI for the real fresh recording, retaining `--no-ship`. |
| 1 | `spawn_and_resume_children_carry_their_own_vendor_harness_version` | Fake Codex implements the sandbox protocol by executing the real probe. |
| 2 | Both BUG-1607 Codex reviewer launch tests | Same protocol; check metadata prefix and retain exact vendor argv comparison. |
| 1 | `bug_2185_standalone_reviewer_commits_child_store_writes_on_its_error_exit` | Same protocol; retain missing-verdict error and child-store commit assertions. |
| 12 | BUG-2149/2150 collection/lifecycle tests | Same protocol; retain own-child positives, sibling/foreign/launcher/stale/wrong-head/freshness/FIFO negatives and cleanup assertions. |

The four broader STORY-1917 train-policy fixture failures overlapping BUG-2401
are a separate retained evidence obligation. BUG-2401 implementation is
serialized after this rework; no concurrent fixture edits or assertion waivers
are assumed. BUG-2403 projection/release-grant work remains separate and
blocked downstream. No ledger edits or older-approval searches are used.

## Validation

Results will be filled from this round's actual gate logs before publication.
The previous host native pass (1/0, 6.08 seconds) is evidence only for the old
source. Earlier nested socket-mask and cache/IO timeout failures remain valid
failed attempts. Model-free host proof does not establish first model-backed
tool metadata or full native parity. Full CI and fresh independent STRICT
review remain required; this implementer does not record review or merge.
