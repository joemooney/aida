# Project Constitution

<!--
Representative GitHub Spec Kit `/speckit.constitution` output.
Hand-authored for the AIDA composition-seam demo (TASK-875) because the
`specify` CLI is not installed in this environment. Structure mirrors the
documented Spec Kit format; content is generic (a small auth service).
-->

## Principles

1. **Library-first.** Every feature is a self-contained library with a thin CLI/HTTP shell. No business logic in the transport layer.
2. **Test-first.** No implementation code lands before a failing test that pins the behavior. Red, then green.
3. **Explicit contracts.** Every public function documents its inputs, outputs, and error modes. No implicit globals.
4. **Simplicity.** Prefer the boring, well-understood option. Add a dependency only when the cost of not having it is concrete.

## Constraints

- Language: Rust (stable toolchain).
- Storage: a single Postgres instance; no per-feature databases.
- Auth tokens are opaque, server-side-revocable (no stateless JWT secrets in v1).

## Governance

This constitution supersedes feature specs on conflict. Amendments require an entry in the changelog and a version bump of this file.

**Version:** 1.0.0 | **Ratified:** 2026-06-01
