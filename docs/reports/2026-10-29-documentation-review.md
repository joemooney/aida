# Documentation freshness and navigation review

Date: 29 October 2026. Reviewed stack base: `b2c511eb9eec6d4932c9a2ba04ad3f2818517c90` (operator documentation slice). Post-fix working-tree hashes identify the content in this PR; the base identifies its Git history context. This is an author audit for independent review, not a merge approval or a declaration that the epic has landed.

<!-- trace:TASK-B6B1278890774EF993D2EBC327862470-113 | ai:codex -->

## Coverage and method

The inventory covers 1,016 tracked documentation inputs: every `docs/` path and Markdown, reStructuredText and AsciiDoc elsewhere in the repository. It includes source templates, generated mirrors, assets and dated evidence. The four self-derived audit outputs are enumerated separately in the snapshot to avoid self-hash cycles. No runtime/untracked store data is included.

Classification totals: 35 current reader references, 10 current static pages, 14 site sources/assets, 213 template masters, 117 derived agent mirrors, 358 historical/evidence inputs, eight decision records, 21 contextual evidence documents, 166 references requiring context, and 74 documentation tools/assets. Classification describes context and intended use; it does not certify the accuracy of every sentence.

The post-fix scan examined 2,591 local links. There are **zero candidates in current reader references or static pages**. The remaining 223 candidates are retained for contextual verification: 86 template-master references, 47 historical/evidence references, 88 references requiring context and two documentation-tool/asset references. Template-relative and historical links can be valid in their original context; these are candidates, not 223 confirmed defects. External URLs are not fetched. Code fences, inline code and template placeholders are excluded from ordinary navigation checking. Candidate line numbers are navigation aids; confirmed evidence below was inspected in source.

Substantive source/help checks cover the changed newcomer/operator routes, init profile and MCP behavior, clone allocation, transport, cache labels, modes/overrides, reporting/effort attribution and the seven curated guides across this PR stack. The site scripts, accessible names, keyboard controls, responsive rules, source provenance and generated navigation were inspected. Unchanged legacy/backend prose, external vendor setup and every historical statement were not re-executed or certified.

## Confirmed issue groups and bounded fixes

Eleven issue groups: one critical, eight important, two minor. The Git diff is the before/after record; obsolete documents are not copied into a second archive.

| Severity | Evidence | Before | After |
|---|---|---|---|
| Important | [First project](../first-project.md):53,69,86; `aida init --help`; allocation authority source | Guaranteed a shared global ID sequence, implicit MCP registration and an initialization role assumption | Illustrative IDs use actual CLI output; MCP opt-in is explicit; validated authority remains separate |
| Important | [CLI journey](../cli/README.md):73; init help/source | MCP registration described as a default | Explicit opt-in and PATH-based profile selection |
| Critical | [Administrator guide](../admin-guide.md):7 | Developer guide path missing | Link to the current repository guide |
| Important | [First project](../first-project.md), Where to go next | Comparison link targeted a removed README anchor; newcomer description was command-centric | Current positioning index and conversational newcomer route |
| Important | [Lifecycle](../lifecycle.md):6,813 | Removed README lifecycle anchors | Current product-workflow guide |
| Important | [Setup reference](../cli/07-project-setup.md):185 | Sanitized-intake anchor used an obsolete heading order | Anchor matches the current source heading |
| Important | [Generated glossary](../cli/12-glossary.md):28,424,426,563 | Pack-relative filenames absent in the manual directory | Two compatibility navigation pages link to existing generated sections; no definitions duplicated and no generated/template content hand-edited |
| Important | [README](../../README.md):16; cache source | Unconditional cache reconstruction wording | Reconstruction scoped to available canonical history |
| Important | [Overview](../what-is-aida.md) and HTML; reviewed commit `9bb1164fe1e` / merged PR #3093 | Stack started before that independent merge | Carried forward the reviewed selectable-autonomy content and styling, preserving the later human-inclusive introduction and Metrics corrections |
| Minor | [Overview](../what-is-aida.md):76 and HTML equivalent; commit metadata | Historical counts could read as current | Explicit commit date and historical-snapshot label in both formats |
| Minor | [Library master](../showcase/library-template.html):4, landing page and library | Complete assessment not directly discoverable from the final site journey | Visible link to the readable assessment, with its capabilities, integrations, offline operation and limitations |

Source evidence includes [init](../../aida-cli-lib/src/init_cmd.rs), [allocation](../../aida-core/src/allocation_authority.rs), [cache reads](../../aida-core/src/db/cached_git_backend.rs), [transport dispatch](../../aida-cli-lib/src/transport_profile.rs), [recoverable pull](../../aida-cli-lib/src/transport_pull.rs), [mode dispatch](../../aida-cli-lib/src/do_dispatch.rs), [effort calibration](../../aida-cli-lib/src/effort_calibration.rs) and [token attribution](../../aida-cli-lib/src/token_ledger.rs). Earlier slices contain the focused implementation/source notes for their own corrections.

## Product coverage and output ownership

The landing page and library reach conversational onboarding, configuration, the collaboration/data loop, selectable autonomy, trust, Metrics and operator setup. The complete assessment supplies the remaining detailed capability, integration, offline-operation and experience context. Human participation and configured background intent capture remain the main entry journey. CLI mutation commands belong primarily to agents/operators.

Seven tracked Markdown guides are the curated publication set in `docs/showcase/publication.json`; broader CLI/history publication remains the recorded advisor curation question. Static guide/library/search output is derived from those sources and shared masters. No historical/decision content was rewritten, no hosted deployment or audience change occurred, and no runtime/gate policy was introduced.

## Verification and reproduction

- `python3 docs/showcase/build.py` regenerated the source-derived site. Unchanged-input rebuild stability and seven source hashes were checked.
- `python3 docs/audit-documentation.py --base b2c511eb9eec6d4932c9a2ba04ad3f2818517c90` regenerates the inventory, candidates and snapshot. The helper is a reporting tool, not a new CI or runtime gate.
- `make check-ci-fast` is the required repository gate. Its completed result and spawned author self-check findings are recorded in the PR/spec handoff. No direct cargo build/test was run.
- Review the bounded before/after diff with `git diff b2c511eb9ee..HEAD -- README.md docs/`.

Artifacts: [inventory](2026-10-29-documentation-inventory.csv), [link candidates](2026-10-29-documentation-links.csv), [snapshot](2026-10-29-documentation-snapshot.json).

## Remaining gaps and integration follow-up

The 223 contextual candidates require their source/publication context before changing them. Historical evidence stays dated. The 166 context-dependent references have no blanket freshness certification. External links, live installations for every vendor, browser/assistive-technology behavior and production deployments were not exhaustively re-tested; source inspection is not a substitute for those checks.

The content PRs are stacked and remain subject to independent review/integration. The independently merged overview changes from PR #3093 are carried forward in this audit, including a human-inclusive resolution of the introductory paragraph. Retargeting the stack still requires ordinary independent integration review. PR #3090 was still open at `eb306b090cd8b71bf1d9c0c0658c7befa10d7eab` during inspection; the trust page's post-land source check remains an integration follow-up. No approval or authority for that lane is supplied by this audit.
