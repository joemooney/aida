# SPIKE-47 — Defining 'trace coverage' for a diff

- **Date**: 2026-06-06
- **Specs**: SPIKE-47 (deliverable), STORY-499 (the gate this gates), EPIC-34 (parent), STORY-498 (the validity-gate MVP that ships alongside)
- **Status**: Complete — research deliverable; go/no-go below
- **Complexity**: research spike (no Rust)

---

## 0. Why this spike exists

EPIC-34 promotes AIDA's headline guarantee — *code traces to a live requirement* — from a client-side, message-grammar courtesy check into a server-side, non-bypassable merge gate. The epic names four gaps:

1. **Format, not coverage** — the hook checks the commit *message* parses, not that the *diff* carries `// trace:` over the code it changed.
2. **Trailer, not validity** — the `(REQ-ID)` is a string match; nothing confirms it's live.
3. **Local hook, not server gate** — bypassable with `--no-verify`, never runs in CI.
4. **Soft by default** — warns rather than blocks.

STORY-498 (Completed) already shut **#2 + #3**: `aida trace gate` walks a commit range, resolves every `(SPEC-ID)` trailer against the live graph, and exits non-zero on a dead/dangling reference — server-side, non-bypassable.

Gap **#1 — coverage** — is the hard, undecided one. A naïve "every changed line needs a `// trace:`" gate would false-positive on **every** legitimate test, fmt, rename, and docs PR and would fight the team daily. The risk is not building it; the risk is building it on a *guessed* definition. This spike settles the definition deterministically **before** STORY-499 implements the gate.

The four open questions from the spec, answered:
- What is a *coverable unit*? → §2
- Which categories are *exempt*? → §3
- How do we *measure* without false positives? → §4
- *Hard-gate or report-only*, and is the dial per-repo? → §5

---

## 1. Definitions (vocabulary this document fixes)

| Term | Meaning |
|---|---|
| **Diff** | The set of changes a PR adds over its merge base (`origin/<default>..HEAD`), excluding merge commits. The same range `aida trace gate` already resolves via `resolve_gate_range`. |
| **Hunk** | A contiguous block of changed lines in one file (a unified-diff `@@ … @@` section). |
| **Coverable unit** | A unit of change that *should* carry trace provenance (§2). The atom the gate scores. |
| **Trace anchor** | A `// trace:<SPEC-ID>` comment (any comment syntax) that resolves to a live spec, *in scope* for a coverable unit (§4.2). |
| **Covered** | A coverable unit that has at least one in-scope trace anchor **or** whose commit carries a live `(SPEC-ID)` trailer (§4.3). |
| **Exempt** | A change that is excluded from the denominator by an explicit, audited rule (§3) — never silently dropped. |
| **Coverage ratio** | `covered_units / (coverable_units − exempt_units)`. The gate compares this (or its per-unit pass/fail form) to the configured posture (§5). |

**Trace-anchor grammar** (already implemented, reused verbatim — `aida-cli/src/main.rs` `trace_re`):
```
trace:([A-Z]+(?:-[A-Z0-9]+)?-[0-9]+(?:-[0-9]+)?)
```
optionally followed by `| ai:<tool>[:<confidence>]`. This is comment-syntax-agnostic — it matches `// trace:`, `# trace:`, `/// trace:`, `<!-- trace: -->` — because it keys off the `trace:` token, not the comment leader. The gate does **not** invent new parsing.

---

## 2. The coverable unit — what requires a `// trace:`

### 2.1 Decision: the **changed source hunk**, attributed to a spec, is the unit — NOT the function and NOT the line.

Three candidate granularities were on the table (from the spec's first open question):

| Granularity | Verdict | Why |
|---|---|---|
| **Per changed line** | Rejected | Punishingly noisy; one trace comment cannot decorate every line; forces comment spam that degrades the signal. |
| **Per function/method** | Rejected for v1 | Requires a language-aware parser (tree-sitter or equivalent) per language to find function boundaries. High build cost, language-by-language drift, and the convention is explicitly language-*agnostic*. Revisit only if hunk-level proves too coarse in practice. |
| **Per changed hunk in a source file** | **Chosen** | Deterministic from `git diff` alone (no language parser). A hunk is the natural "I changed this region of code for a reason" atom. One `// trace:` near a hunk, or a commit trailer, covers it. Coarse enough to not spam, fine enough to catch an untraced feature drop. |

A **coverable hunk** is a hunk that:
1. adds or modifies lines (pure deletions are exempt — §3), **and**
2. lives in a **source file** (an extension in the configured source set; default `rs`, mirroring `aida trace scan --extensions`, extensible per-repo), **and**
3. is not in an exempt file or commit (§3).

### 2.2 Language-awareness is deferred, not denied

v1 is **hunk-level and language-agnostic** by deliberate choice. The trace-anchor regex already works for any comment syntax, so a Python/TS/Go shop gets coverage by adding its extensions to the source set — no parser. Per-function granularity (and per-language parsers) is a named, explicit follow-up (§6), gated on evidence that hunk-level is too coarse. We do not pay the per-language-parser cost on spec.

---

## 3. The exemption list (the denominator-shrinking rules)

Every exemption is **explicit and audited** — the gate *reports* what it exempted and why, never silently drops a change. This matches EPIC-34's acceptance ("exempt by an explicit, audited rule, not silently skipped") and STORY-498's existing posture (plan-commit + no-trailer exemptions are logged, not hidden).

Exemptions compose in two layers: **commit-level** (the whole commit is exempt) and **file/hunk-level** (specific paths or hunks within a non-exempt commit).

### 3.1 Commit-level exemptions (reuse STORY-498's machinery)

| # | Category | Rule | Rationale | Source of truth |
|---|---|---|---|---|
| C1 | **No-trailer mechanical / release commit** | A commit with no `(SPEC-ID)` trailer carries nothing to validate — exempt. | Matches the existing "no REQ-ID needed" rule for `chore`/`docs`/release commits. | `is_plan_commit_subject` sibling logic + TASK-488 exemption already in `validate_trailer_references`. |
| C2 | **Plan commit** | `docs(plans): …` subjects are skipped. | The trailer names what is *planned*, not what shipped; the plan file is not traced code. | `is_plan_commit_subject` (already implemented). |
| C3 | **Release / version-bump / scaffold commit** | `scripts/release.sh` and `aida init` scaffold commits. | TASK-488 audited mechanical-commit exemption, already honoured by `aida trace gate`. | TASK-488. |
| C4 | **Revert / merge commit** | `--no-merges` already drops merges; `revert:` subjects exempt. | A revert restores prior traced state; nothing new to trace. | git log `--no-merges` (already used). |

### 3.2 File / path-level exemptions

| # | Category | Match rule (deterministic) | Rationale |
|---|---|---|---|
| F1 | **Tests** | Path matches a configured test glob: default `**/tests/**`, `**/*_test.*`, `**/*.test.*`, `**/test_*.py`, and Rust `#[cfg(test)]`-only changed files (heuristic: file under `tests/` or hunk inside a `mod tests`). | Tests assert behaviour of already-traced code; they are not themselves the requirement-bearing change. Forcing traces on tests doubles comment noise for zero provenance gain. |
| F2 | **Generated code** | Path matches a generated glob (`**/generated/**`, `*.gen.*`, `aida-generate-types` output, `target/**`) **or** the file's first lines contain a generated-marker (`@generated`, `DO NOT EDIT`, `Auto-generated by`). | By definition not hand-authored against a requirement; the generator's source is the traceable artifact. |
| F3 | **Docs / prose** | Non-source extensions: `md`, `txt`, `rst`, `adoc`, and the `docs/**` tree. | Docs are not code; they carry no `// trace:` convention. (The doc *content* may reference a SPEC-ID in prose, but that is not a coverage obligation.) |
| F4 | **Config / data / lockfiles** | `*.lock`, `*.toml`/`*.yaml`/`*.json` data files, `.gitignore`, CI YAML, vendored manifests. | Declarative config is not requirement-bearing code; lockfiles are mechanical. |
| F5 | **Vendored / third-party** | `vendor/**`, `node_modules/**`, `third_party/**`. | Not our authored code. |

### 3.3 Hunk-level exemptions (within a non-exempt source file)

| # | Category | Match rule (deterministic) | Rationale |
|---|---|---|---|
| H1 | **Pure deletion** | A hunk with only `-` lines (no `+`). | Removing code carries no new provenance obligation. |
| H2 | **fmt-only / whitespace-only** | A hunk whose `+`/`-` lines are identical after whitespace normalization (the `git diff -w` test: the hunk vanishes under `--ignore-all-space`). | `cargo fmt` / prettier reflows are mechanical; CLAUDE.md already treats `style`/`fmt` as REQ-ID-optional. |
| H3 | **Pure rename / move** | git rename-detection (`git diff -M`) reports the file as a rename with no content delta beyond the path. | Moving traced code does not change what it traces. |
| H4 | **Trivial one-liner / comment-only** | A hunk whose net change is ≤ 1 effective line, OR whose `+` lines are *only* comments/blank. | Below the noise floor; a one-char typo fix or a comment edit is not a requirement-bearing unit. Configurable threshold (default 1; per-repo `[trace_coverage] trivial_max_lines`). |

**Exemption-stacking guard.** Because exemptions only *shrink the denominator*, a buggy over-broad exemption *weakens* the gate, it never falsely blocks. That asymmetry is intentional: we bias toward false-negatives (a real untraced change slips) over false-positives (a legit PR is blocked), because false-positives are what kill a gate's adoption. The validity gate (STORY-498) is the hard backstop; coverage is the softer net.

---

## 4. Measurement — attributing a hunk to a trace anchor without false positives

### 4.1 Inputs (all deterministic, all from git — no store needed for the diff walk)

1. The commit range (`resolve_gate_range`, already implemented).
2. `git diff -M -w <base>..HEAD` with `--unified=0` for precise hunk boundaries, plus a full-context pass to find nearby trace anchors.
3. The post-change file content (to scan for `// trace:` anchors that *survive* in the merged tree, not just appear in the `+` lines).
4. The live requirement graph (only to confirm a found anchor/trailer resolves Live — reuses STORY-498's `SpecResolution` resolver).

### 4.2 Attribution rule — when does an anchor "cover" a hunk?

A coverable hunk is **covered** if **any** of these hold (cheapest first):

1. **In-hunk anchor** — a `// trace:<live-id>` appears in the hunk's `+` lines.
2. **Proximity anchor** — a `// trace:<live-id>` exists in the *post-change file* within **N lines above** the hunk start (default N=5, configurable). This captures the dominant real pattern: one `/// trace:STORY-X` doc comment above a function whose body changed. Searching *above only* (not below) matches the Rust/AIDA convention where the trace sits on the item it annotates.
3. **File-level anchor** — a module-level `//! trace:<live-id>` anywhere in the post-change file covers every hunk in that file. Matches the existing `//!` module-trace pattern (`aida-core/src/*.rs` headers).
4. **Commit-trailer fallback** — the hunk's commit carries a live `(SPEC-ID)` trailer. This is the **floor**: a PR whose commits all carry valid trailers (which STORY-498 *already* enforces for validity) is, by this rule, *fully covered* even with zero inline `// trace:`.

### 4.3 The trailer-fallback is the key false-positive killer

Rule 4.2(4) makes the coverage gate **a strict superset of nothing the team isn't already doing**. Every feat/fix commit *already* needs a `(REQ-ID)` trailer (CLAUDE.md + STORY-498). So in the default world, **every non-exempt hunk is covered by its own commit's trailer** and the coverage gate is *silent*. Inline `// trace:` only becomes load-bearing when someone wants finer-than-commit attribution, or under a stricter posture (§5) that requires inline anchors for high-blast-radius files.

This is why the gate will not "fight every PR": its default denominator-vs-numerator is already satisfied by the trailer discipline STORY-498 enforces. The coverage gate's marginal job is catching the case where a commit's trailer claims SPEC-X but the diff *also* slips in unrelated untraced code — i.e., trailer scope-creep — and even that is **report-only** by default (§5).

### 4.4 Determinism guarantees

- No NLP, no heuristics beyond the explicit globs/regex above.
- Same diff + same graph snapshot ⇒ same verdict, on any machine.
- The anchor regex, the trailer parser, the `SpecResolution` resolver, and `resolve_gate_range` are **already shipped** (STORY-498) and unit-tested — STORY-499 reuses them and adds only the diff-walk + attribution + exemption layers.

---

## 5. Default posture — block vs report

### 5.1 Recommendation: **report-only by default, block opt-in, per-repo dial — mirroring `AIDA_COMMIT_STRICT`.**

| Posture | Behaviour | Default? |
|---|---|---|
| **`report`** | Annotate the PR with the coverage ratio + the list of uncovered coverable hunks. CI job **succeeds**. | ✅ **Default** |
| **`block`** | Same report; CI job **fails** if any coverable hunk is uncovered (ratio < threshold). | Opt-in. |
| **`off`** | Gate does not run. | — |

Config surface (per-repo, mirrors the existing `[telemetry]`/`AIDA_COMMIT_STRICT` pattern):
```toml
[trace_coverage]
posture = "report"        # report | block | off
source_extensions = ["rs"]
proximity_lines = 5
trivial_max_lines = 1
# threshold = 1.0          # block only below this ratio (1.0 = every unit must be covered)
```
Env override: `AIDA_TRACE_COVERAGE=report|block|off` (matches `AIDA_COMMIT_STRICT`, `AIDA_TELEMETRY`).

### 5.2 Why report-first, not block-first

1. **A coverage gate's failure mode is adoption death.** A gate that false-positives on a real PR gets `--no-verify`'d, then disabled, then resented. Report-only earns trust by being *right* before it's *binding*. (CLAUDE.md memory: *substrate-as-bouncer* works only when the bouncer is calibrated; an over-eager bouncer gets removed.)
2. **The validity half (STORY-498) is already a hard block** — the dangerous hole (a dead/hallucinated provenance link) is shut. Coverage is the *completeness* refinement, which is lower-stakes and benefits from a calibration period.
3. **The dial mirrors a pattern operators already understand** (`AIDA_COMMIT_STRICT` warn-vs-reject). Consistency lowers the conceptual cost.
4. **Report-mode is a free calibration harness.** Running it report-only for a few weeks across real PRs surfaces the exemption-list gaps (the false-positives) *without* blocking anyone — exactly the data needed to safely flip a given repo to `block`.

This repo (AIDA) should run `report` for ≥ 2 weeks, mine the uncovered-hunk reports for missed exemptions, then flip to `block` once the false-positive rate is ~zero.

---

## 6. Go / No-Go

### **GO — build STORY-499, report-only first.**

The definition is now precise enough to implement deterministically:
- **Unit**: the changed source hunk (§2).
- **Exemptions**: an explicit, audited, two-layer list (§3) reusing STORY-498's commit-level rules.
- **Measurement**: four-tier attribution with the commit-trailer floor as the false-positive killer (§4).
- **Posture**: report-by-default, block opt-in, per-repo dial mirroring `AIDA_COMMIT_STRICT` (§5).

The build cost is modest because STORY-499 **reuses** the shipped STORY-498 machinery (range resolver, trailer parser, anchor regex, `SpecResolution` resolver) and adds only: a `git diff` hunk walker, the exemption matchers, the proximity-anchor scan, and a report/annotation formatter. No language parser, no NLP, no store schema change.

### Smallest valuable first slice (for STORY-499)

> **`aida trace coverage [--range R] [--json]` — report-only, hunk-level, with the §3 exemptions and the §4.2 four-tier attribution, run as a non-blocking PR CI step that annotates the coverage ratio + uncovered hunks.**

That slice delivers EPIC-34 gap #1's *visibility* immediately, is non-blocking so it can't fight a PR, and produces the calibration data to later justify flipping `posture = "block"`. The `block` posture and the `[trace_coverage]` config keys are a thin follow-up once the report has run clean.

### Explicitly deferred (named follow-ups, not v1)

- **Per-function granularity / per-language parsers** (§2.2) — only if hunk-level proves too coarse in practice.
- **`block` posture wiring + threshold config** — after the report-only calibration period.
- **Per-file stricter posture** (require inline `// trace:` for high-blast-radius paths) — a later refinement of the dial.

### Risks / gotchas the implementer must respect

1. **Bias to false-negatives over false-positives.** Over-broad exemptions weaken the gate; over-narrow ones kill adoption. When unsure, exempt — the validity gate is the hard backstop.
2. **Anchor scope is *above-only* + module-level** (§4.2) — do not match a `trace:` that sits *below* a hunk (it annotates the next item, not this one).
3. **Use the post-change file for anchor scanning, not just `+` lines** — an unchanged `/// trace:` above a changed body must still count (§4.2 rule 2).
4. **Resolve anchors/trailers against the *live* graph** — a `// trace:DEAD-ID` is not coverage; reuse `SpecResolution` so a rejected/nonexistent id doesn't count as covered (consistency with STORY-498).
5. **Honour `--no-merges` and the same `resolve_gate_range` defaults** STORY-498 uses, so the two gates scan an identical commit range.

---

## 7. Related

- **EPIC-34** — parent epic (the four-gap framing).
- **STORY-498** (Completed) — validity gate; the machinery STORY-499 reuses (`validate_trailer_references`, `SpecResolution`, `resolve_gate_range`, `is_plan_commit_subject`, anchor `trace_re`).
- **STORY-499** (Approved, blocked-by SPIKE-47) — the gate this spike unblocks; implement per §6's first slice.
- **TASK-488** — the audited mechanical-commit exemption reused at the commit level (§3.1).
- CLAUDE.md "Code traceability" — the `// trace:<SPEC-ID> | ai:<tool>` convention and the commit-format/`AIDA_COMMIT_STRICT` dial this posture mirrors.
- Memory: `feedback_substrate_as_bouncer_not_rules.md` — the principle the report-first calibration posture operationalizes (a programmatic gate must be *calibrated* to stay a bouncer rather than get removed).
