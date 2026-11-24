---
marp: true
title: "AIDA — Deterministic Engineering × Agent Code Review"
subtitle: "How AIDA Combines Requirement Graphs, Clean-Room Rust Constraints, and Multi-Seat Agent Intelligence"
author: "Joe Mooney"
date: "November 2026"
description: "Architecture, benchmark, and dogfood case study of AIDA's native deterministic code review engine (STORY-3076)."
paginate: true
theme: default
---

<!--
RENDER:
  npx @marp-team/marp-cli@latest docs/internal/presentation/aida-code-review-engine.md -o aida-code-review-engine.html
  npx @marp-team/marp-cli@latest docs/internal/presentation/aida-code-review-engine.md --pdf -o aida-code-review-engine.pdf
  pandoc docs/internal/presentation/aida-code-review-engine.md -t pptx -o docs/showcase/aida-deterministic-code-review.pptx
AUDIENCE: Engineering leadership, staff engineers, and AI architecture teams.
GOAL: Explain why raw LLM code review fails at scale, how AIDA's clean-room Rust review engine (`aida review prepare`) solves it, and show the 6 real bugs it caught when reviewing its own implementation.
trace:STORY-3076 | ai:antigravity
-->

# Deterministic Engineering × Agent Review

### Beyond "Vibe-Check" AI Code Review in AIDA

Combining **Requirement Graphs**, **Clean-Room Rust Constraints**, and **Multi-Seat Agent Intelligence** (`STORY-3076`)

::: notes
Welcome. Today we are walking through a major upgrade to AIDA's assurance layer: our native, clean-room Rust deterministic code review preparation and verification engine (`aida review prepare`). We will cover why raw prompt-based AI code review fails on real pull requests, the four deterministic pillars we built into `aida-core` and `aida-cli-lib`, and what happened when we turned the engine loose on its own 2,000-line Rust implementation.
:::

---

# The Problem with Raw Agent Code Reviews

## Why general-purpose LLM review skills break on real changesets

- **Incomplete file coverage**: On multi-file PRs, general-purpose agents skim the middle of the context window and silently skip files.
- **Line-number position drift**: LLMs routinely miscount unified diff line numbers, anchoring comments to the wrong lines.
- **Prompt & quality variance**: Generic natural-language checklists ("check error handling and concurrency") degrade into shallow pass/fail guesses.
- **Intent blindness**: Standalone code-review tools inspect syntax and diffs in a vacuum — they do not know what requirement the change was supposed to satisfy.

::: notes
If you hand a 2,000-line diff to a general-purpose coding agent with a prompt skill, four failure modes appear almost immediately: the agent cuts corners on coverage, drifts on line numbers, forgets language-specific edge cases unless prompted with exact failure patterns, and has no connection to the canonical requirement graph.
:::

---

# Core Philosophy: "Substrate as Bouncer"

## Deterministic Rust handles what *must not go wrong*

| Responsibility | Deterministic Rust Substrate (`aida review prepare`) | Independent Reviewer Seat (Claude / Codex / Antigravity) |
|---|---|---|
| **Scope & Exclusions** | Resolves `merge-base..HEAD` or `--scan`, filters lockfiles/secrets/substrate, seals `inventory_sha256` | Never wastes tokens on `Cargo.lock` or `.aida-store` |
| **Batching** | Pairs `<stem>.rs` + `<stem>_test.rs` into bounded `<=12k` token batches | Reviews each module + its tests in focused context |
| **Rules & Playbooks** | Matches path globs + injects diff-triggered ANSSI / Tokio / Rust API facets | Evaluates concrete failure patterns against reachable call sites |
| **Intent & Acceptance** | Lifts linked spec `## Acceptance` criteria + verifies `// trace:<SPEC-ID>` | Judges whether every acceptance criterion has falsifiable evidence |
| **Positioning & Coverage** | Anchors quoted `existing_code` to exact `start_line..end_line` & gates on 100% file coverage | Quotes defective snippets and records defect class + verdict |

::: notes
Our design principle is "Substrate as Bouncer": never ask an LLM to do mechanical bookkeeping that Rust can guarantee deterministically. Rust computes the inventory hash, pairs source and test files into token-bounded batches, selects the exact security and concurrency checklists triggered by the diff, anchors quoted code snippets to exact line numbers, and refuses any review ledger with less than 100% file coverage.
:::

---

# Pillar 1: Sealed Inventory & 100% Coverage Gate

## Every changed file is classified and accounted for

- **Deterministic Classification (`classify_diff_inventory`)**:
  - Evaluates workspace changes, `--commit <sha>`, `--from <base> [--to <head>]`, `--pr <N>`, or full-tree `--scan --path <dir>`.
  - Classifies excluded files with explicit reasons: `binary_file`, `pure_deletion`, `generated_or_lockfile`, `aida_substrate`, `secret_sensitive_path`, or `user_excluded:<glob>`.
  - Seals the review scope with a canonical **SHA-256 inventory digest** (`inventory_sha256`) and flags whether each file carries an inline `trace:<SPEC-ID>` comment.
- **Hard Coverage Verification (`--verify-coverage <ledger.json>`)**:
  - `verify_coverage_ledger` checks that **100% of `reviewable_files`** are explicitly marked `reviewed` or `skipped` (with a mandatory non-empty rationale).
  - Unaccounted files or blank skip reasons fail closed with a non-zero exit code.

::: notes
First pillar: no more silent file skipping. `aida review prepare` classifies every file in the diff, explains why any file was excluded, seals the inventory with a SHA-256 digest, and exposes `--verify-coverage` so an approval cannot land if the reviewer skipped a single reviewable file without explanation.
:::

---

# Pillar 2: Zero-LLM Semantic & Token-Budgeted Batching

## Pairing implementation with tests inside a 12,000-token ceiling

- **Companion Test Pairing (`canonical_module_stem`)**:
  - Automatically normalizes stems across `<name>.rs`, `<name>_test.rs`, `<name>_tests.rs`, `test_<name>.py`, and `<name>.test.ts`.
  - Pairs implementation changes with their unit/integration tests into a single `module:<stem>` batch so missing negative tests are spotted immediately.
- **Crate-Qualified Generic Stems**:
  - Qualifies entry-point filenames (`lib.rs`, `mod.rs`, `main.rs`, `index.ts`) by parent directory (`aida-core/src/lib` vs `aida-cli-lib/src/lib`) so unrelated crates never collide.
- **Token & File-Count Bin Packing (`bundle_review_batches`)**:
  - Packs remaining files by shared playbook ID up to `DEFAULT_BATCH_TOKEN_BUDGET` (`12,000` tokens) and `DEFAULT_MAX_FILES_PER_BATCH` (`8` files).

::: notes
Second pillar: how do you keep an agent's attention sharp on a 30-file PR without making an extra LLM call just to group files? We use a deterministic stem normalizer that pairs implementation files with their companion test files first, qualifies generic names like `lib.rs` by their parent crate directory, and bin-packs the rest by playbook within a 12,000-token ceiling.
:::

---

# Pillar 3: Modular Playbooks & Diff-Triggered Facets

## Distilling Rust API, ANSSI, Tokio & Google standards at zero extra overhead

- **Always-On Core Playbook (~250 tokens)**:
  - **Anti-Noise Floor**: Never duplicate `rustc`, borrow-checker, `clippy`, or `rustfmt` diagnostics.
  - **AIDA Defect-Class Mapping**: Directly targets `fail-open`, `absent-evidence-reads-as-good`, `perf`, `portability`, `contract-drift`, `incomplete-fix`, and `untested-path`.
- **Zero-Overhead Diff-Triggered Conditional Facets (`detect_rust_content_facets`)**:
  - Appended **only** when the reviewed files actually contain matching constructs:
    1. `rust_unsafe_ffi` (*ANSSI / Rustonomicon*): Triggered by `unsafe`, `extern "C"`, `libc::`, `from_raw_parts`, `MaybeUninit`.
    2. `rust_async_concurrency` (*Tokio Guidelines*): Triggered by `async fn`, `.await`, `Mutex`, `RwLock`, `Atomic`, `select!`.
    3. `rust_io_subprocess_security` (*ANSSI*): Triggered by `Command::new`, `std::fs`, `Path`, `serde_json`, `toml::`.
    4. `rust_macros`: Triggered by `macro_rules!` or `proc_macro`.

::: notes
Third pillar: prompt specificity drives recall, but a 300-line static prompt wastes tokens and dilutes attention. Our solution is a compact 5-bullet Core Playbook synthesized from the Rust API Guidelines, ANSSI Secure Rust, Tokio, and Google Engineering Practices, paired with 4 conditional facets that only inject into the prompt when the target files actually touch `unsafe`, async/locks, subprocess/filesystem I/O, or macro definitions.
:::

---

# Pillar 4: Snippet Line-Anchoring & Citation Verification

## Eliminating LLM line-number drift and unverifiable claims

- **Two-Pass Deterministic Line Anchoring (`--anchor-findings <findings.json>`)**:
  - Reviewers quote the exact defective lines in `existing_code` instead of guessing line numbers.
  - `anchor_snippet_lines` runs a sliding-window search over the revision-pinned file (`git show <sha>:<path>`):
    - **Pass 1**: Exact line-by-line match (disambiguated by minimum distance to optional `start_line` hint).
    - **Pass 2**: Trimmed-whitespace fallback that tolerates LLM indentation drift while recording `anchor_status: "trimmed_whitespace"`.
- **Observation Evidence Citations (`verify_observation_citations`)**:
  - Validates every `[[obs-NNNNNN]]` marker cited in a review report against the run's observation log, flagging unknown or errored observation IDs.

::: notes
Fourth pillar: LLMs are great at quoting the defective code block they see, and terrible at arithmetic over diff hunks. With `--anchor-findings`, the agent quotes `existing_code` and Rust deterministically resolves the exact 1-based `start_line` and `end_line` on the target commit SHA, while `verify_observation_citations` ensures every `[[obs-N]]` evidence tag resolves to a real, non-errored observation.
:::

---

# Case Study: Reviewing Itself (`STORY-3076`)

## 6 real bugs caught and fixed when the engine reviewed its own ~2,200 lines of Rust

| # | Defect Class | What Was Caught During Self-Review | How `--anchor-findings` & Fixes Landed |
|---|---|---|---|
| **1** | `maintainability` | `canonical_module_stem` paired `aida-core/src/lib.rs` and `aida-cli-lib/src/lib.rs` into `module:lib` | Anchored to `review_engine.rs:551..554`; qualified generic stems (`lib`, `mod`, `main`) by parent dir |
| **2** | `bug` / `incomplete-fix` | `git diff --numstat` lacked `--no-renames` and called `parts.last().unwrap()` in production code | Anchored to `review_prepare.rs:336..339`; added `--no-renames` and slice pattern matching |
| **3** | `security` | `git rev-parse` and `git merge-base` omitted second-layer `END_OF_OPTIONS` (`--end-of-options`) | Anchored to `review_prepare.rs:280..285`; added `END_OF_OPTIONS` across all git calls |
| **4** | `fail-open` / `absent-evidence-reads-as-good` | `collect_git_diff_entries` swallowed non-zero `git diff` exit codes as an empty 0-file inventory | Caught by fail-closed error semantics rule; now fails closed via `anyhow::bail!` |
| **5** | `security` / `contract-drift` | `read_file_at_revision` fell back from a commit SHA to the dirty working tree and lacked `ParentDir` checks | Caught by `rust_io_subprocess_security` facet; removed worktree fallback on pinned SHAs & blocked `..` |
| **6** | `perf` | `verify_observation_citations` recompiled `Regex::new` on every call | Caught by allocations rule; replaced with `once_cell::sync::Lazy<Regex>` |

::: notes
Here is the proof that the architecture works. As soon as `aida review prepare` compiled, we ran it on its own 2,200-line worktree and compared it head-to-head with standalone review tooling. Together, the batching output, the fail-closed error semantics rule, and the `rust_io_subprocess_security` facet surfaced 6 real bugs—including a fail-open subprocess check and a revision-to-working-tree fallback—and `--anchor-findings` pinpointed the exact line numbers with 100% accuracy.
:::

---

# Architectural Comparison: Why Native in AIDA Wins

## 100% clean-room Rust inside AIDA vs. external standalone review binaries

| Capability | Standalone External CLI Review Tools | AIDA Native (`aida review prepare` + `/aida-review`) |
|---|---|---|
| **Provenance & Supply Chain** | External third-party binary + Node wrapper | **100% clean-room Rust** inside `aida-core` & `aida-cli-lib` |
| **API Keys & Cost** | Requires separate LLM provider setup for full pipeline | **Zero extra keys**: runs across Claude Code, Codex, and Antigravity seats |
| **Requirement Graph Integration** | None (requires shell glue + truncated text files) | **Native**: reads `.aida-store`, extracts `## Acceptance`, checks `// trace:` |
| **Batching in Host-Agent Mode** | Dumps all files of a language into one flat group | **Deterministic token-budgeted batches** + source/test pairing in all modes |
| **Playbook Token Efficiency** | Static 75-line language prompt on every file | **Core (~250 tok) + Diff-Triggered Conditional Facets** (zero bloat when absent) |
| **Diff + Whole-Module Audits** | Separate commands (`review` vs `scan`) | Unified in `aida review prepare` (`--pr`, `--from`, `--commit`, or `--scan --path`) |

::: notes
By building this cleanly inside `aida-core` and `aida-cli-lib`, we eliminated third-party binary provenance risk, removed the need for separate LLM API keys, brought token-budgeted batching and exact line anchoring to host-agent reviews, and connected code review directly to AIDA's requirement graph and defect-class ledger.
:::

---

# Getting Started & CLI Reference

## Built into `/aida-review`, `/aida-code-review`, and the `aida` CLI

```bash
# 1. Prepare a PR or branch range review with linked spec acceptance criteria
aida review prepare --pr 42 --write .aida/review-prepare-pr-42.md
aida review prepare --from main --specs STORY-3076

# 2. Audit an existing crate or directory without a Git diff (Full-File Scan Mode)
aida review prepare --scan --path aida-core/src --write .aida/code-review-prepare.md

# 3. Deterministically verify 100% file coverage and anchor quoted code snippets to exact lines
aida review prepare --pr 42 \
  --verify-coverage .aida/review-coverage.json \
  --anchor-findings .aida/review-findings.json \
  --json --write .aida/review-verified.json
```

- **Customize per project**: Add path exclusions, batch token budgets, and custom path rules in `.aida/review-rules.toml`.

::: notes
Everything shown here is live in the `aida` CLI and wired directly into AIDA's `/aida-review` and `/aida-code-review` skill templates. Thank you — happy to take questions or walk through a live terminal run.
:::
