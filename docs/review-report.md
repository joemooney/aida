# Documentation review and product-site refresh

Reviewed: 2026-10-29. Owning requirement: EPIC-b6b1278890774ef993d2ebc327862470-002.

## Scope and meaning of this review

The initial inventory covers **608 tracked files under docs/**: 124 current/derived documents, 408 historical/evidence files, 37 decision/evidence files and 39 assets. [The inventory](review-inventory.tsv) records every initial path, category, last committed date and size. Root README.md and OVERVIEW.md were also checked and updated.

The exhaustive part is the inventory and corpus-wide mechanical screening: local file links, stale-platform/storage/identity phrases, source paths, tone candidates and documentation categories. The substantive review concentrates on current product/onboarding claims, configuration and authority boundaries, command references and the material used by the site. This is not a claim that every statement in 608 files or every external vendor UI was independently re-tested.

Historical plans, research and archive contents remain dated evidence. Generated graph documentation is not hand-edited. The glossary was corrected through its generator, then regenerated and checked. External client matrix rows retain their actual verification dates; this review does not relabel them as newly verified.

## Product and audience decisions

- People communicate outcomes, resolve decisions and inspect results through an AIDA-aware agent. The CLI is primarily an agent interface, also available for operator setup, inspection and troubleshooting.
- Position AIDA as a governed software factory with selectable autonomy. The four execution modes are operator, guided, drive and drain. Decide is a separate block. Launch/presence flags are not additional execution modes.
- Configurability is a primary capability: agent defaults, editable type/lane protocols, per-spec autonomy, workflow policy, project footprint and reporting. Settings have feature-specific trust scopes, not a universal precedence rule.
- Current alpha/Linux, harness differences, shared Unix-user trust boundary and merge-versus-deployment limits remain explicit.

## Confirmed issue families and corrections

| Severity | Location | Before | After / verification |
|---|---|---|---|
| Critical | getting-started.md, multi-hub-sync.md | Darwin downloads and four release artifacts despite Linux-only support | Linux x86_64/arm64 setup and release boundary; checked README, installer and repository platform policy |
| Important | README.md, OVERVIEW.md, docs/README.md, first-project.md | Human expected to file, edit and queue routine specs by hand | Human/agent conversation journey, with separately labeled operator commands |
| Important | WHY-AIDA.md | 15 Claude-only skills; YAML/PostgreSQL-first pitch; GitHub and non-Claude support described as absent | Current Git-backed factory model, multi-vendor surfaces, traceability and explicit limitations |
| Important | admin-guide.md | Old GUI/database settings and legacy SQL procedures mixed into primary administration | Scoped data layers, configuration map, supported maintenance/migration references |
| Important | using-aida-with-claude-code.md | Production drain described as compiled workflow experiment; MCP and fixed skill counts treated as defaults | Current CLI-first agent surface, explicit MCP opt-in, configurable launch and separation from historical experiments |
| Important | UNDERSTANDING_SKILLS.md | Skills described as passive knowledge only | Procedural agent guidance distinguished from invocation surfaces and deterministic enforcement |
| Important | multi-node.md, storage-modes.md, CLI setup and developer deck | Sequential-node acquisition and block consumption described without current authority restriction | Opaque ordinary-clone binding, explicit HOME authority and non-authoritative block refusal |
| Important | 11 current documents | 19 moved aida-cli/src Rust references | Changed only where old file is absent and matching aida-cli-lib/src file exists |
| Important | Current Markdown links | 9 missing-file references, including generated glossary cross-links | Current local file links resolve; source-template links or generated intra-page anchors replace missing packs |
| Important | lifecycle.md, autonomous-drain.md | Three presence/launch variants called the autonomy modes | Explicit distinction from the four typed execution choices plus decide |
| Important | configuration trust reference | Vendor hard-coded and original field list described as exhaustive | Configurable launches and maintained trust inventory linked, without granting config authority |
| Minor | Presentation/vision entry points, MCP matrix | Old counts and vendor setup could be mistaken for fresh capability claims | Dated context and current product/reference routes; original per-row dates retained |
| Minor | multi-user-setup.md | Unqualified 100K+ requirements capacity statement | Workload-dependent capacity, no claimed benchmark |

## Evidence and checks

- CLI manual guard: all 84 help-all commands documented; cited subcommands and flags exist; all 14 surface-changing specs reflected; no prohibited spec-ID noise.
- Glossary generation and --check pass; links are transformed at generation time, not patched into generated output only.
- Current local Markdown file references checked after the rewrites: zero missing targets. Illustrative placeholder anchors and historical links are not interpreted as current product navigation.
- Source evidence includes ExecutionMode and RequirementType, do_dispatch human contracts, work_binding and allocation_authority, per-agent launch configuration, configuration trust inventory, protocol documentation and existing review/merge policy.
- This does not execute mutating command examples against production state, certify every historical command, benchmark performance, or verify external vendor client matrices anew.

Additional corrections: the power-features page no longer promises unlimited concurrent fire-and-forget drains; cache guidance distinguishes canonical data from bounded/deferred read refresh; the review page distinguishes review topologies from execution modes and names the restricted solo-human exception. Unqualified lossless migration language was removed.

## Full before/after record

[review-changes.txt](review-changes.txt) contains the complete current-documentation diff for this pass, including the new configuration guide (excluding the separate site implementation, inventory and this report). Files in that diff:

- `OVERVIEW.md`
- `README.md`
- `docs/README.md`
- `docs/SUMMARY.md`
- `docs/UNDERSTANDING_SKILLS.md`
- `docs/WHY-AIDA.md`
- `docs/admin-guide.md`
- `docs/agents/aida-mcp-install-matrix.md`
- `docs/agents/aida-repository-guide.md`
- `docs/agents/claude-surfaces-codex-parity.md`
- `docs/agents/cross-agent-onboarding.md`
- `docs/aida-power-features.md`
- `docs/api/aida-core.md`
- `docs/architecture/autonomy-and-escalation.md`
- `docs/architecture/mcp-coordination-surface.md`
- `docs/architecture/one-orchestration-engine.md`
- `docs/autonomous-drain.md`
- `docs/cli/03-work-autonomy.md`
- `docs/cli/07-project-setup.md`
- `docs/cli/09-integrations.md`
- `docs/cli/10-storage.md`
- `docs/cli/12-glossary.md`
- `docs/cli/README.md`
- `docs/cli/generate-glossary.sh`
- `docs/environment-variables.md`
- `docs/first-project.md`
- `docs/future-vision.md`
- `docs/getting-started.md`
- `docs/lifecycle.md`
- `docs/mailbox.md`
- `docs/multi-hub-sync.md`
- `docs/multi-node.md`
- `docs/multi-user-setup.md`
- `docs/positioning/README.md`
- `docs/positioning/when-not-to-use-aida.md`
- `docs/presentation/2026-06-management-demo.md`
- `docs/presentation/aida-administrator-guide.md`
- `docs/presentation/aida-developer-deep-dive.md`
- `docs/presentation/aida-executive-briefing.md`
- `docs/presentation/aida-user-walkthrough.md`
- `docs/review-process.md`
- `docs/security/config-trust-boundary.md`
- `docs/session-lifecycle.md`
- `docs/storage-modes.md`
- `docs/using-aida-with-claude-code.md`
- `docs/what-is-aida.html`
- `docs/what-is-aida.md`
- `docs/configuration.md`

## Product site and maintenance

The site includes product workflow, autonomy, configuration, trust, human-first getting started, a printable brief and a searchable library generated from current/reference Markdown. Original SVG graphics explain the human/agent loop, configuration layers, trust path and data layers. Research, plans, archived comparisons and old decks are not silently presented as current product pages.

Run `python3 docs/showcase/build.py` after source documentation changes. [The site README](showcase/README.md) documents source ownership and deployment. Search indexes the documentation content, not only titles. The hosted audience remains owner-private.

The repository still contains dated material by design. This pass gives it an explicit place and removes it from the main product journey instead of rewriting history to match today's product.

## Site verification

The eight main/brief pages were checked in Chromium at 390, 768 and 1440 pixels with no document-level horizontal overflow or JavaScript errors. Smoke checks covered all four autonomy modes, the six configuration panels, keyboard selection/wrapping, all policy examples, copy-request feedback, mobile menu/Escape, content search, category filtering, empty results and the brief print action. Local page/asset links and rendered section anchors were checked across the generated site.

Contrast findings were corrected and the main/brief pages rechecked with axe-core WCAG A/AA and WCAG 2.1 AA rules: no automated violations were detected. This is a scoped automated check, not a claim of full accessibility certification. Desktop and mobile pages were also visually inspected.

All 104 integrated reference pages were also opened at mobile width. One long-heading overflow in the repository guide was corrected and rechecked. The generated product-library routes point to the integrated pages rather than requiring a GitHub detour.

## Human-inclusive intent framing and Metrics tab — 2026-10-29

Added Metrics to shared navigation and footer, with reporting availability, coverage limits, derived/proposed measures and source references. The overview and its standalone HTML now lead with people and agents building together and keeping both the implementation and durable intent. Onboarding starts with normal conversational coding and configured background capture; full delivery orchestration is optional. The homepage, workflow, start page and executive brief carry that framing. A coverage map links the overview’s principal topics into the product site.

Corrected stale effort-estimate prose against requirements_cli.rs (filing), queue_cmd_args.rs (pickup) and effort_calibration.rs. Dated repository metric examples remain explicitly historical. Updated a stale autonomy link exposed by the full generated-site anchor check.
