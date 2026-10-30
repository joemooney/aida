# Documentation freshness and navigation review

Date: 29 October 2026. Reviewed main base: `1db74dba1129632c01b5bae9ee5681dd76069db1` (restacked after the prerequisite overview/showcase merges). Post-fix working-tree hashes identify the content in this PR; the base identifies its Git history context. This is an author audit for independent review, not a merge approval or a declaration that the epic has landed.

<!-- trace:TASK-B6B1278890774EF993D2EBC327862470-113 | ai:codex -->

## Coverage and method

The inventory covers 1,017 tracked documentation inputs: every `docs/` path and Markdown, reStructuredText and AsciiDoc elsewhere in the repository. It includes source templates, generated mirrors, assets and dated evidence. The four self-derived audit outputs are enumerated separately in the snapshot to avoid self-hash cycles. No runtime/untracked store data is included.

Classification totals: 37 current reader references, 10 current static pages, 15 site sources/assets, 213 template masters, 117 derived agent mirrors, 358 historical/evidence inputs, eight decision records, 21 contextual evidence documents, 164 references requiring context, and 74 documentation tools/assets. Classification describes context and intended use; it does not certify the accuracy of every sentence.

The post-fix scan examined 2,591 local links. There are **zero candidates in current reader references or static pages**. The remaining 220 candidates are retained for contextual verification: 86 template-master references, 47 historical/evidence references, 85 references requiring context and two documentation-tool/asset references. Template-relative and historical links can be valid in their original context; these are candidates, not 220 confirmed defects. External URLs are not fetched. Code fences, inline code and template placeholders are excluded from ordinary navigation checking. Candidate line numbers are navigation aids; confirmed evidence below was inspected in source.

Substantive source/help checks cover the changed newcomer/operator routes, init profile and MCP behavior, clone allocation, transport, cache labels, modes/overrides, reporting/effort attribution and the seven curated guides across this PR stack. The site scripts, accessible names, keyboard controls, responsive rules, source provenance and generated navigation were inspected. Unchanged legacy/backend prose, external vendor setup and every historical statement were not re-executed or certified.

## Confirmed issue groups and bounded fixes

Eleven issue groups: one critical, eight important, two minor. The Git diff is the before/after record; obsolete documents are not copied into a second archive.

| Severity | Evidence | Before | After |
|---|---|---|---|
| Critical | [First project](../first-project.md):49–52,65,80–86; `aida init --help`; allocation authority source | Guaranteed a shared global ID sequence, implicit MCP registration and an initialization role assumption | Illustrative IDs use actual CLI output; MCP opt-in is explicit; validated authority remains separate |
| Important | [CLI journey](../cli/README.md):73; init help/source | MCP registration described as a default | Explicit opt-in and PATH-based profile selection |
| Important | [Administrator guide](../admin-guide.md):7 | Developer guide path missing | Link to the current repository guide |
| Important | [First project](../first-project.md), Where to go next | Comparison link targeted a removed README anchor; newcomer description was command-centric | Current positioning index and conversational newcomer route |
| Important | [Lifecycle](../lifecycle.md):6,813 | Removed README lifecycle anchors | Current product-workflow guide |
| Important | [Setup reference](../cli/07-project-setup.md):185 | Sanitized-intake anchor used an obsolete heading order | Anchor matches the current source heading |
| Important | [Generated glossary](../cli/12-glossary.md):28,424,426,563 | Pack-relative filenames absent in the manual directory | Two compatibility navigation pages link to existing generated sections; no definitions duplicated and no generated/template content hand-edited |
| Important | [README](../../README.md):16; cache source | Unconditional cache reconstruction wording | Reconstruction scoped to available canonical history |
| Important | [Overview](../what-is-aida.md) and HTML; merged PR #3093 | Stack started before that independent merge | Replayed only the eight slice commits onto current main, preserving main's opening sentence and selectable-autonomy content alongside conversational onboarding and Metrics corrections |
| Minor | [Overview](../what-is-aida.md):76 and HTML equivalent; commit metadata | Historical counts could read as current | Explicit commit date and historical-snapshot label in both formats |
| Minor | [Library master](../showcase/library-template.html):4, landing page and library | Complete assessment not directly discoverable from the final site journey | Visible link to the readable assessment, with its capabilities, integrations, offline operation and limitations |

Source evidence includes [init](../../aida-cli-lib/src/init_cmd.rs), [allocation](../../aida-core/src/allocation_authority.rs), [cache reads](../../aida-core/src/db/cached_git_backend.rs), [transport dispatch](../../aida-cli-lib/src/transport_profile.rs), [recoverable pull](../../aida-cli-lib/src/transport_pull.rs), [mode dispatch](../../aida-cli-lib/src/do_dispatch.rs), [effort calibration](../../aida-cli-lib/src/effort_calibration.rs) and [token attribution](../../aida-cli-lib/src/token_ledger.rs). Earlier slices contain the focused implementation/source notes for their own corrections.

## Product coverage and output ownership

The landing page and library reach conversational onboarding, configuration, the collaboration/data loop, selectable autonomy, trust, Metrics and operator setup. The complete assessment supplies the remaining detailed capability, integration, offline-operation and experience context. Human participation and configured background intent capture remain the main entry journey. CLI mutation commands belong primarily to agents/operators.

Seven tracked Markdown guides are the curated publication set in `docs/showcase/publication.json`; broader CLI/history publication remains the recorded advisor curation question. Static guide/library/search output is derived from those sources and shared masters. No historical/decision content was rewritten, no hosted deployment or audience change occurred, and no runtime/gate policy was introduced.

## Verification and reproduction

- `python3 docs/showcase/build.py` regenerated the guide/library/search output. Unchanged-input rebuild stability and seven source hashes were checked.
- `python3 docs/audit-documentation.py --base 1db74dba1129632c01b5bae9ee5681dd76069db1` regenerates the inventory, candidates and snapshot. The helper is a reporting tool, not a new CI or runtime gate.
- `make check-ci-fast` is the required repository gate. Its completed result and spawned author self-check findings are recorded in the PR/spec handoff. No direct cargo build/test was run.
- Review the bounded before/after diff with `git diff 1db74dba1129632c01b5bae9ee5681dd76069db1...HEAD -- README.md docs/`.

Artifacts: [inventory](2026-10-29-documentation-inventory.csv), [link candidates](2026-10-29-documentation-links.csv), [snapshot](2026-10-29-documentation-snapshot.json).

## Remaining gaps and integration follow-up

The 220 contextual candidates require their source/publication context before changing them. Historical evidence stays dated. The 164 context-dependent references have no blanket freshness certification. External links, live installations for every vendor, assistive-technology behavior, full browser interactions and production deployments were not exhaustively re-tested; source inspection is not a substitute for those checks.

The eight content PRs were rebuilt on current main after advisor feedback and remain subject to fresh independent review at their new heads. Pre-squash prerequisite commits were not replayed. Main's exact overview opening sentence was retained. PR #3090 is merged; the trust page's statements were checked against the landed verdict/mandate sources: independent review at the current head, the audited human exception, separate CI/hold checks and validated mandate scope remain accurate. This audit grants no review or merge authority.

## Advisor correction pass

The correction pass also restored clone-local `aida init` / `aida node acquire` before creating specs, the allocation retry and duplicate-ID refusal details, and queue freshness/unpublished-store commands. It removed duplicate onboarding copy, restored an uninstall section, corrected trust/autonomy links, updated Metrics pricing limitations and Appendix B notes, and replaced stale publication process notes with manifest ownership guidance. The documentation index and positioning links now reach existing anchors/files.

The standalone overview HTML is restored byte-for-byte from this PR's base branch. The Pandoc regeneration route has been removed from this slice; the ordinary guide/library build continues to use Python's standard library plus Git. Its checks do not regenerate or certify the standalone overview. No runtime or protected gate implementation was modified.

Author self-check found a stale default-mode paragraph in the linked review-process guide denying the existing solo-human exception. That paragraph and the introductory absolute wording now distinguish ordinary independent review from the verified, explicitly enabled human exception. The exception never authorizes an implementing agent's self-approval. No accessibility certification of the standalone overview is claimed.

## Round-2 verification

The onboarding routes now describe the integrated conversational guide, and rendered assessment links preserve their section fragments. The internal Getting started button has no external-link glyph. The setup references require clone initialization before spec creation, describe bounded cache freshness, and restore per-user queue inspection and stable identity configuration.

The [publication builder](../showcase/build.py) collects guide, library and search output in memory. `python3 docs/showcase/build.py --check` compares all tracked derived artifacts without writing them and reports every drifted file. The existing static CI step runs that check and its behavioral fixtures. The standalone assessment is outside this builder and its checks; ordinary guide checks have no Pandoc dependency. Duplicate/reserved output names refuse, search retains legitimate punctuation and link labels, footer provenance is a template field, and Unicode excerpts use offsets from the original displayed string.

The [page template](../showcase/page-template.html) places the source heading before the graphic; image descriptions come from the SVG title and description. Decorative configuration-link arrows are hidden from assistive technology. The showcase README lists every generated guide and data artifact, and the configuration guide explains protocol inspection and seeding before showing a protocol.

The issue table gives onboarding/allocation/MCP misstatements priority over the broken administrator link. The reproduction commands pin the reviewed base and use a three-dot comparison. The regenerated inventory and candidate scan cover the final source set; the local fixtures cover drift/no-write behavior, output collisions, search, provenance, graphic descriptions/heading order. These are author checks, not independent approval or an assistive-technology audit.
