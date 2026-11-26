# Inspect what allows a change to advance

<!-- trace:TASK-B6B1278890774EF993D2EBC327862470-109 | ai:codex -->

AIDA's governed workflow makes evidence and authority explicit. Ask what was agreed, which change was checked, who checked it and what authorizes integration. The answers come from several records with different scopes, not from selecting a delivery mode.

## Intent and implementation

Acceptance criteria describe the agreed outcome. Code traces and spec-bearing commits connect implementation to that intent. A trace grants no approval or completion credit by itself. An author checks the change before handoff; that check is preparation for independent review.

## Independent review at a commit

The ordinary review path requires a reviewer independent of the change's author. Both `/aida-review` and `/aida-code-review` use `aida review prepare` to build their file inventories. The command inventories files, groups likely companions by stem, selects facets by whole-file substring matches and anchors quoted snippets. The optional CLI ledger check accounts for each reviewable file as reviewed or skipped with a reason. Merge gates operate independently. Verdict evidence identifies the reviewed commit, and shipping evaluates applicable evidence against the current head. An old approval is not permission to ship arbitrary later work.

There is an explicit, opt-in and audited solo-human self-approval exception for a verified person at a terminal. This does not turn an implementing agent's self-check into independent approval. The [review process](review-process.md) and current review command help describe the exact evidence and exception contracts.

## CI and holds

Applicable required checks and merge holds are separate from the review judgment. A green CI result alone is not review approval or authority to clear a hold. Holds and their clearance rules preserve reasons that require additional action; a tool refusal should be resolved through the authorized path.

This page explains the existing controls. It does not promise every rule is unbypassable by a user who controls the machine or the forge, and it does not introduce a new clearance mechanism.

## Grants and mandates

Role grants provide scoped, validated authority. An environment variable naming a role is not itself a grant. A person at an interactive terminal can start and stop a time-boxed mandate; an eligible advisor action still needs the mandate's scope and applicable evidence. Current mandate help documents an eight-hour default and 24-hour maximum, and operator-only work remains outside mandate coverage.

Protected paths and other mandatory human gates remain outside what a mandate can authorize. For example, the runtime gate directory and project authority configuration have protected-path treatment. Sensitive work has additional scope rules; inspect the current mandate and command help instead of assuming that all held work is covered.

[Who may do what](advanced/authority-and-seats.md) explains how roles, seats, grants, delegation, mandates and the deputy fit together, with diagrams.
<!-- trace:TASK-2606 | ai:claude -->

## Know the boundary

AIDA is alpha software and Linux-only. Current identity and local file controls operate under a shared Unix user; a same-UID process that forges trusted local files is outside the threat model. Multi-human identity separation is not a security boundary. Harness capabilities and containment support differ, and configuration is not a universal isolation guarantee.

Merging a change is separate from release and production deployment. A project must supply its own deployment policy, credentials and automation. Choosing drain does not grant access to production or prove an application meets its operational obligations.

## Specification-mediated clean-room engineering

When AIDA adopts architectural ideas or evaluation methodologies from external ecosystems, academic research, or industry standards, it uses the requirement specification in `.aida-store` as a **clean-room firewall**:

1. **Observe functional concepts only (`/aida-ecosystem-scan`)**: The Advisor studies external tools, papers (such as GitHub ReviewBench), and public standards (such as OASIS SARIF 2.1.0, ANSSI Secure Rust, and OWASP/CWE) to understand the underlying problem and mathematical or behavioral invariants.
2. **Distill pure acceptance criteria (`aida add`)**: The requirement specification records only unprotectable functional requirements, mathematical formulas, and AIDA-specific invariants (`## Acceptance`)—never foreign source code, data structures, or prompt prose.
3. **Implement from scratch in native Rust**: The Implementer builds directly from the AIDA specification using existing workspace crates (`aida-core`, `aida-cli-lib`) with `// trace:<SPEC-ID>` markers and zero added third-party runtime dependencies or copied prompt templates. This eliminates copyright/license contamination, prevents derivative-work exposure, and closes the supply-chain and trojan-prompt attack surface. See [About & clean-room methodology](showcase/about.html) and the [Clean-Room IP & Supply-Chain Assurance audit](showcase/code-review.html#slide-cleanroom) for the complete architecture blueprints and subsystem-by-subsystem breakdown.

## Read the records at their own scope

The [workflow guide](product-workflow.md) separates code history, canonical intent, the cache and local evidence. Store synchronization does not transfer every operational artifact or machine grant. Review, CI, authority and hold records need the validation defined by their own commands.

For operators, inspect `aida mandate status`, `aida role status` and the relevant review/PR command help. Contributors can read the [verdict implementation](../aida-cli-lib/src/gate/review_verdict.rs) and [mandate gates](../aida-cli-lib/src/gate/mandate_gates.rs). These are evidence references, not invitations to edit protected controls. Return to [selectable autonomy](selectable-autonomy.md) to choose the human involvement contract.
