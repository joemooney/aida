# AIDA product showcase

A presentation for newcomers and engineering management, centered on **a governed software factory with selectable autonomy**.

- Open [index.html](index.html) for the interactive showcase.
- Open [executive-brief.html](executive-brief.html) for a concise meeting handout. Its Print / Save as PDF button uses the browser's print dialog.
- Read [the detailed overview](../what-is-aida.md) for supporting technical context and limitations.

The site is buildless and self-contained: HTML, CSS, JavaScript and an inline SVG diagram. It has no external font, package, analytics or image dependency. Both pages work when opened directly from disk; deploy this directory with any static host. Publishing to a Git repository alone does not make the HTML a website.

The autonomy selector supports mouse, touch, arrow keys, Home and End. With JavaScript disabled, the initial autonomous-delivery explanation remains readable. Print styles are included, and reduced-motion preferences are respected.

The diagrams explain the operating model; they are not a live dashboard. Descriptive delivery labels map to existing execution modes rather than introducing new CLI values. Claims are grounded in `ExecutionMode` in `aida-core/src/models.rs`, the human contracts in `aida-cli-lib/src/do_dispatch.rs`, and the existing overview. Keep the alpha/Linux and authority limitations accurate when editing the copy.

The initial version was also published as an owner-private preview through Sites. Access to that preview is separate from these portable source files; sharing the preview URL does not make it public.

<!-- trace:TASK-b6b1278890774ef993d2ebc327862470-101 | ai:codex -->
