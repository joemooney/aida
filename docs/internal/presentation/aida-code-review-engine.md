---
marp: true
title: "AIDA — Deterministic Engineering × Agent Code Review"
subtitle: "How AIDA Combines Requirement Graphs, Clean-Room Rust Constraints, and Multi-Seat Agent Intelligence"
author: "Joe Mooney"
date: "November 2026"
description: "Architecture, feature matrix, and dogfood case study of AIDA's native deterministic code review engine (STORY-3076)."
paginate: true
theme: default
---

<!--
RENDER:
  npx @marp-team/marp-cli@latest docs/internal/presentation/aida-code-review-engine.md -o aida-code-review-engine.html
  npx @marp-team/marp-cli@latest docs/internal/presentation/aida-code-review-engine.md --pdf -o aida-code-review-engine.pdf
AUDIENCE: Engineering leadership, staff engineers, and AI architecture teams.
GOAL: Explain review preparation using an illustrative six-defect case study.
trace:STORY-3076 | ai:antigravity
-->

# Deterministic Engineering × Agent Review

### Upcoming with CLI slice S2 and review skills slice S3

Combining **Requirement Graphs**, **Clean-Room Rust Constraints**, and **Multi-Seat Agent Intelligence** (`STORY-3076`)

<!-- The command is upcoming with CLI slice S2. Skill integration follows in S3. -->

---

# The Problem with Raw Agent Code Reviews

## Why general-purpose LLM review skills break on real changesets

- **Incomplete file coverage**: On multi-file PRs, general-purpose agents skim the middle of the context window and silently skip files.
- **Line-number position drift**: LLMs routinely miscount unified diff line numbers, anchoring comments to the wrong lines.
- **Prompt & quality variance**: Generic natural-language checklists ("check error handling and concurrency") degrade into shallow pass/fail guesses.
- **Intent blindness**: Standalone code-review tools inspect syntax and diffs in a vacuum — they do not know what requirement the change was supposed to satisfy.

<!-- Large diffs can hide skipped files and make line references hard to check. -->

---

# Core Philosophy: "Substrate as Bouncer"

## Deterministic Rust handles what *must not go wrong*

| Responsibility | Deterministic Rust Substrate (`aida review prepare`) | Independent Reviewer Seat (Claude / Codex / Antigravity) |
|---|---|---|
| **Scope & Exclusions** | Resolves `merge-base..HEAD` or `--scan`, filters lockfiles/secrets/substrate, seals `inventory_sha256` | Never wastes tokens on `Cargo.lock` or `.aida-store` |
| **Batching** | Groups likely companions by stem, targeting 12k estimated tokens | Reviews each module + its tests in focused context |
| **Rules & Playbooks** | Selects playbooks by path and Rust facets by substring matches across each file | Evaluates concrete failure patterns against reachable call sites |
| **Intent & Acceptance** | Lifts linked spec `## Acceptance` criteria + flags the literal `trace:` marker | Judges whether every acceptance criterion has falsifiable evidence |
| **Positioning & Coverage** | Anchors quoted `existing_code` to exact `start_line..end_line` and optionally checks a file ledger | Quotes defective snippets and records defect class + verdict |

<!-- Separate mechanical checks from the reviewer’s judgment about intent and defects. -->

---

# Pillar 1: Sealed Inventory & Optional File Ledger

## The inventory distinguishes reviewable and excluded files

- **Deterministic Classification (`classify_diff_inventory`)**:
  - Git is required. Workspace mode uses HEAD and local changes, range mode uses a merge base, PR mode needs forge metadata and local refs, commit mode needs the parent, and scan mode lists tracked and non-ignored untracked files with `git ls-files`.
  - Classifies excluded files with explicit reasons: `binary_file`, `pure_deletion`, `generated_or_lockfile`, `aida_substrate`, `secret_sensitive_path`, `user_excluded:<glob>`, or `oversize: N estimated tokens > cap M`.
  - Seals the review scope with a canonical **SHA-256 inventory digest** (`inventory_sha256`) and flags whether the literal `trace:` occurs, without validating an ID.
- **Optional CLI File Accountability (`--verify-coverage <ledger.json>`)**:
  - `verify_coverage_ledger` checks that **100% of `reviewable_files`** are explicitly marked `reviewed` or `skipped` (with a mandatory non-empty rationale).
  - Unaccounted files or blank skip reasons fail closed with a non-zero exit code.

<!-- A skipped file with a reason satisfies the optional ledger check. Merge authorization is separate. -->

---

# Pillar 2: Zero-LLM Semantic & Token-Budgeted Batching

## Pairing implementation with tests by stem with a 12,000-token estimated target

- **Companion Test Pairing (`canonical_module_stem`)**:
  - Automatically normalizes stems across `<name>.rs`, `<name>_test.rs`, `<name>_tests.rs`, `test_<name>.py`, and `<name>.test.ts`.
  - Uses a same-stem heuristic to identify likely companions. Preparation lists over-cap files and their token estimates in the exclusions and coverage report, then batches the remaining files.
- **Crate-Qualified Generic Stems**:
  - Qualifies entry-point filenames (`lib.rs`, `mod.rs`, `main.rs`, `index.ts`) by parent directory (`aida-core/src/lib` vs `aida-cli-lib/src/lib`) so unrelated crates never collide.
- **Token & File-Count Bin Packing (`bundle_review_batches`)**:
  - Packs remaining files by shared playbook ID toward `DEFAULT_BATCH_TOKEN_BUDGET` (`12,000` tokens) and `DEFAULT_MAX_FILES_PER_BATCH` (`8` files).

<!-- Files with matching stems can be unrelated. Preparation lists over-cap files and their token estimates in the exclusions and coverage report, then batches the remaining files. -->

---

# Pillar 3: Modular Playbooks & Whole-File Substring Facets

## Distilling Rust API, ANSSI, Tokio & Google standards at zero extra overhead

- **Always-On Core Playbook (~250 tokens)**:
  - **Anti-Noise Floor**: Never duplicate `rustc`, borrow-checker, `clippy`, or `rustfmt` diagnostics.
  - **AIDA Defect-Class Mapping**: Directly targets `fail-open`, `absent-evidence-reads-as-good`, `perf`, `portability`, `contract-drift`, `incomplete-fix`, and `untested-path`.
- **Whole-file substring matching (`detect_rust_content_facets`)**:
  - Appended **only** when the reviewed files contain matching substrings, including comments and unchanged lines:
    1. `rust_unsafe_ffi` (*ANSSI / Rustonomicon*): Triggered by `unsafe`, `extern "C"`, `libc::`, `from_raw_parts`, `MaybeUninit`.
    2. `rust_async_concurrency` (*Tokio Guidelines*): Triggered by `async fn`, `.await`, `Mutex`, `RwLock`, `Atomic`, `select!`.
    3. `rust_io_subprocess_security` (*ANSSI*): Triggered by `Command::new`, `std::fs`, `Path`, `serde_json`, `toml::`.
    4. `rust_macros`: Triggered by `macro_rules!` or `proc_macro`.

<!-- A match in unchanged code or a comment can select a facet. -->

---

# Pillar 4: Snippet Line-Anchoring & Citation Verification

## Eliminating LLM line-number drift and unverifiable claims

- **Two-Pass Deterministic Line Anchoring (`--anchor-findings <findings.json>`)**:
  - Reviewers quote the exact defective lines in `existing_code` instead of guessing line numbers.
  - `anchor_snippet_lines` runs a sliding-window search over the revision-pinned file (`git show <sha>:<path>`):
    - **Pass 1**: Exact line-by-line match (disambiguated by minimum distance to optional `start_line` hint).
    - **Pass 2**: Trimmed-whitespace fallback that tolerates LLM indentation drift while recording `anchor_status: "trimmed_whitespace"`.
- **Library-only Observation Evidence Citations (`verify_observation_citations`)**:
  - Validates every `[[obs-NNNNNN]]` marker cited in a review report against the run's observation log, flagging unknown or errored observation IDs.

<!-- The CLI anchors snippets. Citation verification currently has a library interface only. -->

---

# Case Study: Reviewing Itself (`STORY-3076`)

## Six illustrative defects from the review engine implementation

| # | Defect Class | What Was Caught During Self-Review | How `--anchor-findings` & Fixes Landed |
|---|---|---|---|
| **1** | `maintainability` | `canonical_module_stem` paired `aida-core/src/lib.rs` and `aida-cli-lib/src/lib.rs` into `module:lib` | Anchored to `review_engine.rs:551..554`; qualified generic stems (`lib`, `mod`, `main`) by parent dir |
| **2** | `bug` / `incomplete-fix` | `git diff --numstat` lacked `--no-renames` and called `parts.last().unwrap()` in production code | Anchored to `review_prepare.rs:336..339`; added `--no-renames` and slice pattern matching |
| **3** | `security` | `git rev-parse` and `git merge-base` omitted second-layer `END_OF_OPTIONS` (`--end-of-options`) | Anchored to `review_prepare.rs:280..285`; added `END_OF_OPTIONS` across all git calls |
| **4** | `fail-open` / `absent-evidence-reads-as-good` | `collect_git_diff_entries` swallowed non-zero `git diff` exit codes as an empty 0-file inventory | Caught by fail-closed error semantics rule; now fails closed via `anyhow::bail!` |
| **5** | `security` / `contract-drift` | `read_file_at_revision` fell back from a commit SHA to the dirty working tree and lacked `ParentDir` checks | Caught by `rust_io_subprocess_security` facet; removed worktree fallback on pinned SHAs & blocked `..` |
| **6** | `perf` | `verify_observation_citations` recompiled `Regex::new` on every call | Caught by allocations rule; replaced with `once_cell::sync::Lazy<Regex>` |

<!-- The 100% anchoring figure describes the six selected examples. It is illustrative and supplies no measured accuracy rate for other findings. -->

---

# Illustrative Feature Matrix

## OpenCodeReview and the upcoming AIDA workflow

| Capability | Standalone External CLI Review Tools | AIDA Native (`aida review prepare` + `/aida-review`) |
|---|---|---|
| **Provenance & Supply Chain** | External third-party binary + Node wrapper | **100% clean-room Rust** inside `aida-core` & `aida-cli-lib` |
| **Requirement Graph Integration** | None (requires shell glue + truncated text files) | **Native**: reads `.aida-store`, extracts `## Acceptance`, flags the `trace:` marker |
| **Batching in Host-Agent Mode** | Dumps all files of a language into one flat group | **Deterministic token-budgeted batches** + source/test pairing in all modes |
| **Playbook Token Efficiency** | Static 75-line language prompt on every file | **Core (~250 tok) + Whole-File Substring Conditional Facets** (zero bloat when absent) |
| **Diff + Whole-Module Audits** | Separate commands (`review` vs `scan`) | Unified in `aida review prepare` (`--pr`, `--from`, `--commit`, or `--scan --path`) |

<!-- Read this as an illustrative feature matrix for OpenCodeReview and AIDA. The table contains no measured benchmark results. -->

---

# Getting Started & CLI Reference

## Upcoming CLI and skill integration

S2 adds `aida review prepare`. S3 connects it to `/aida-review` and `/aida-code-review`.

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

<!-- Use these examples after the CLI and skills slices land. -->
