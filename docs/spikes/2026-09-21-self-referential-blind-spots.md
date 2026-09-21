# Self-referential blind-spot sweep

**Date:** 2026-09-21

**Spec:** SPIKE-85

**Lane:** research

## Question and result

This sweep asked which production surfaces gate, classify, detect, or report a
category, and whether each surface is exercised against (a) its own
implementation/configuration and (b) documents that describe its category.

The useful predicate is narrower than “anything that checks something”:

> A self-referential surface is production code or configuration whose output
> changes a workflow decision, blocks a transition, or claims to summarize
> operational state, and whose own source/configuration or category-defining
> prose can be supplied as an input without changing the meaning of the check.

This excludes ordinary type checks, business predicates, and tests that only
assert a function's local behavior. It includes CI/release gates, prose/error
classifiers, generated-contract drift detectors, and aggregate status surfaces.

One new actionable failure was found and filed separately as **BUG-1578**:
the docs-only CI optimization suppresses the documentation drift guards on
documentation-only PRs. No product or design decision is made here. Two scope
decisions remain for the advisor at the end of this report.

## Method (repeatable)

1. Enumerate workflow gates from named steps in `.github/workflows/*.yml` and
   release-time checks called by `scripts/release.sh`.
2. Enumerate external-prose classifiers with
   `python3 scripts/external-prose-classifiers.py --check`; the marker contract
   and generated inventory are in `scripts/external-prose-classifiers.py` and
   `docs/architecture/external-tool-output-classifiers.md`.
3. Enumerate operational summaries by searching public command dispatch and
   renderers for `status`, then retain only aggregate surfaces that can
   contradict their own underlying member/detail view.
4. For every retained surface, inspect tests for both a positive case and a
   negative/adversarial case. Then ask two additional questions:
   - does the check consume its own source/configuration, or is there a test
     that mutates the checker and proves the check can fail?
   - does category-describing prose/configuration pass without being mistaken
     for a category instance?
5. Treat “not applicable” as a result only when the surface cannot
   meaningfully accept its own implementation or descriptive prose as input.
6. File each newly actionable defect as its own spec; do not bundle fixes into
   this report.

Re-run commands:

```bash
rg -n '^\s*- name:' .github/workflows/*.yml
python3 scripts/external-prose-classifiers.py --check
python3 -m unittest tests/test_external_prose_classifiers.py
python3 scripts/check-trace-headers.py
python3 -m unittest tests/test_check_trace_headers.py
rg -n 'external-prose-classifier:' aida-*/src -g '*.rs'
rg -n 'DrainStatus|render_.*status|status_segment' aida-cli-lib/src aida-tui/src
```

The predicate cannot completely enumerate semantic classifiers. The external
prose inventory explicitly depends on reviewers adding a marker, and arbitrary
string comparisons do not reveal their data provenance. The sweep therefore
reports both the marker-bounded set and that boundary; silently presenting it
as exhaustive would reproduce the defect under study.

## Inventory and results

### CI and release gates

| Surface | Source-grounded self/category application | Result |
|---|---|---|
| Docs-only/full-CI classifier | `.github/workflows/ci.yml`, steps `Detect full-CI changes` and `CI docs-only short-circuit` | **Fail.** Its excluded category (`docs/**`, Markdown) contains inputs owned by the downstream doc gates, yet those gates also require `full_ci == true`. Directly changing their governed documents avoids the instruments. Filed as BUG-1578. |
| External-prose classifier inventory | `scripts/external-prose-classifiers.py`; `tests/test_external_prose_classifiers.py` | **Partial pass.** Negative controls prove stale output, a newly added marker, and a moved marker fail; the checker compares its own generated document. It cannot discover an unmarked classifier or non-Rust classifier, by stated design. |
| Trace/doc-header stranding gate | `scripts/check-trace-headers.py`; `tests/test_check_trace_headers.py::test_self_test_the_checkers_own_source_is_clean` | **Pass within stated scope.** It scans its own source, has adversarial fixtures, and records why doc-only and cross-file cases are not machine-detectable by this method. |
| Doc-intent gate | `docs/cli/verify-interface-changes.py`; `tests/test_verify_interface_changes.py` | **Pass for code-side category; fail at workflow application.** Tests mutate the gate's own governed path set and category prose is scoped to YAML blocks rather than bare substring search. However docs-only workflow suppression is BUG-1578. |
| CLI-manual drift guard | `docs/cli/verify-manual.py`; CI step `CLI-manual drift-guard` | **Partial.** It reflects the live CLI and checks command/flag citations, so category prose is exercised. A docs-only edit to the manual does not schedule it: BUG-1578. |
| Monitor-contract drift guard | `docs/monitor-contract-fixtures/verify.py`; `tests/test_monitor_contract.py`; `docs/monitor-contract.md` | **Partial.** Fixtures prove additive versus breaking schema changes, including its own contract shape. A docs-only contract edit skips the workflow step: BUG-1578. |
| Portability ratchet | `scripts/check-portability.sh`, `scripts/check-portability-growth.py`, `tests/test_portability_growth.sh`; CI loads the growth checker from the base revision | **Pass after BUG-1443/BUG-1301.** The base-revision checker prevents a PR from weakening the instrument that judges it, and tests include a weakened checker and growth negative control. Rule completeness remains review-owned rather than inferable. |
| Merge-hold gate | `.github/workflows/merge-hold-gate.yml` | **Pass after BUG-1435, conditional on repository settings.** The workflow describes its required-check dependency and runs on label changes. Whether branch protection actually requires it is external configuration and is not self-verifiable from this tree. |
| Commit SPEC-ID validity gate | CI step `Validate commit SPEC-ID references`; `aida trace gate` tests in the Rust suite | **Pass for its category.** The server-side gate evaluates PR commits including a commit that changes the gate. Store availability is established before evaluation. Not applicable to descriptive prose: prose does not assert a commit trailer. |
| Diff trace coverage report | CI step `Report diff trace coverage (report-only)` | **Not a gate.** It is explicitly report-only and ignores failure (`|| true`), so it must not be counted as enforcing its own trace category. Its implementation can be inspected by the report when code changes trigger full CI. |
| Removed-flag spelling guard | `scripts/check-removed-flags.sh`; CI step of the same name | **Pass within lexical scope.** It scans the tree that includes its category documentation; exceptions are explicit. It is skipped for docs-only edits, but removed spellings in docs are part of its governed input, so this is also covered by BUG-1578 rather than a second bug. |
| Glyph-literal lint | `scripts/glyph-lint.sh --block`; CI step `Glyph-literal lint` | **Pass within allow-list scope.** The registry and lint source are explicit exclusions because they define the literals; additions elsewhere are diff-gated. Category documentation is not semantically classified. |
| MCP doc-consistency gate | `tests/test_mcp_doc_consistency.sh` and `.py`; CI step `Run MCP doc-consistency test` | **Pass for tool descriptors; workflow gap is BUG-1578.** It compares documentation to live descriptors, exercising the documents that describe its category. Docs-only changes currently skip it. |
| Advisor code gates | `tests/test_advisor_code_guard.sh`, `tests/test_advisor_code_gate.sh`; corresponding CI steps | **Pass within boundary.** Tests exercise advisor/implementer, mixed changes, empty stage, explicit overrides, and `--no-verify`. Descriptive prose is not an input; the vendor hook and substrate gate test each other at different boundaries. |
| Build/test/fmt/clippy, bwrap live test, TUI seam, distributed-store tests, web build, release artifact verification | `.github/workflows/{ci,web,release}.yml` | **Not applicable as a self-category test.** These execute or compile the artifacts; they do not classify prose or summarize a category. Their own workflow syntax is validated by GitHub Actions before execution, but that is platform validation rather than an AIDA self-test. |
| Release docs and cross-platform freshness gates | `scripts/docs-gate.sh`, `scripts/pre-release-check.sh`, `scripts/release.sh`, `.github/workflows/cross-platform.yml` | **Partial.** Docs regeneration is checked for idempotence and version presence; cross-platform freshness has no-runs/date-parsing regression tests. Skip flags are explicit policy escapes. External GitHub schedule/branch configuration cannot be established from the tree alone. |

### External-prose classifiers

The marker-bounded inventory currently contains 21 sites. All are enumerated
from production Rust by `scripts/external-prose-classifiers.py`; their source
paths below are the generated contract, not a hand-maintained rediscovery.

| Sites | Source | Self/category result |
|---|---|---|
| `auto_complete::{is_database_locked_message,is_environmental_failure,is_merge_conflict_failure}` | `aida-cli-lib/src/auto_complete.rs` | Adversarial positive/negative unit cases exist. **Not applicable** to own source: inputs are external error text, not Rust or category docs. Descriptive wording is not treated as runtime stderr. |
| `cache::is_cache_schema_drift_error` | `aida-core/src/db/cache.rs` | Unit cases cover matching and non-matching database errors. **Not applicable** to source/docs for the same typed-input reason. |
| `claude_log_indicates_api_outage`, `gh_stderr_is_network_error`, `status_context::load_pr_facts` | `aida-cli-lib/src/lib.rs` | Error/log classifiers have positive and negative fixtures. `load_pr_facts` is the weak member: it is marked but has no same-symbol unit reference outside its definition, so its behavior is covered only through higher-level status tests. This is evidence for a test gap, not yet evidence of a defect. |
| `compete::parse_gate_result` | `aida-cli-lib/src/compete.rs` | Tests exercise accepted verdict vocabulary and malformed output. Category prose is intentionally not accepted because parsing is bounded to the agent response protocol. |
| `forge::{glab_stderr_is_transient,gitlab_merge_response_is_stale_head,gitlab_merge_response_is_retryable}` | `aida-cli-lib/src/forge.rs` | Positive/negative unit cases exist for external CLI/HTTP prose. Own Rust/docs are not valid inputs. |
| `git_ops::{looks_like_index_lock_failure,commit,push,restore_stranded_autostash}` | `aida-core/src/git_ops.rs` | Failure wording is covered with fixtures; commit/push decisions are integration boundaries. Own source/category prose is not process output, so self-application is not meaningful. |
| `network_retry::classify_transient` | `aida-cli-lib/src/network_retry.rs` | Broad table-driven positives and negatives exercise the vocabulary and avoid a bare category-term match. Own source/docs are not runtime errors. |
| `overlay::ci_style` | `aida-tui/src/overlay.rs` | Unit cases cover known CI states and fallback styling. Its input category is structured status text, not descriptive prose. |
| `pr_cmd::pr_fetch_failure_message`, `pr_ship::classify_gh_pr_checks_registration` | `aida-cli-lib/src/{pr_cmd,pr_ship}.rs` | Positive and negative GitHub CLI outputs are covered. Own source/docs are not valid command output. |
| `remote_create::gitlab_release_body_exists` | `aida-cli-lib/src/remote_create.rs` | Fixtures cover present/absent release-body shapes. Own source/docs are not API output. |
| `terminal_cmd::run_terminator_command` | `aida-cli-lib/src/terminal_cmd.rs` | Tests cover recognized command failures and fallback behavior. Its classifier consumes process results, so source/docs are not meaningful inputs. |

The common limitation is important: 20 of 21 marked sites have direct
same-symbol test references, but the inventory proves declaration, not test
quality or completeness. It also cannot prove that every semantic classifier
was marked. Turning that into a claim would require a policy decision about a
closed registry or language-level wrapper, not a lint inferred from strings.

### Aggregate status surfaces

| Surface | Source-grounded self/category application | Result |
|---|---|---|
| `aida drain status` human/TOON/JSON | `aida-cli-lib/src/drain_state.rs::{probe,render_toon_with_context,render_json_with_context}` and the module's unit tests | **Pass after BUG-1441.** Tests cover none, active, stale, single/batch progress, exclusion counts, and JSON/TOON status words from one `DrainStatus` model. Detail and aggregate renderers share state rather than independently rediscovering membership. |
| TUI drain panel | `aida-tui/src/redesign/drain_panel.rs` | **Partial.** It deliberately reads the same `.aida/drain-state.json` payload and resolves the same main-worktree root, reducing split-brain risk. No end-to-end assertion compares a live CLI rendering with the TUI member table; that is a coverage gap, not a demonstrated contradiction. |
| Monitor JSON contract/watch feed | `docs/monitor-contract.md`, contract fixtures, and `tests/test_monitor_contract.py` | **Pass within versioned subset.** The contract declares the read-only fields and fixtures test compatibility. It does not claim to expose all internal state, so omitted non-contract fields are not contradictions. |
| `aida statusline` / maintenance status fields | `aida-cli-lib/src/lib.rs` and `maintenance_schedule.rs` status field collectors | **Not applicable to category prose; partial self-check.** Unit tests cover individual field rendering and missing-state fallbacks. There is no single member table with which the compact footer promises equality, so the BUG-1441 contradiction predicate does not apply. |

## Findings and non-findings

1. **BUG-1578 (new, draft):** one workflow condition suppresses multiple
   documentation instruments precisely on their governed category. This is one
   mechanism and one fix boundary, so it is one spec rather than four.
2. The external-prose inventory is honestly bounded but not exhaustive. Its
   reliance on review-added markers is a known epistemic boundary, not a newly
   demonstrated bug.
3. `status_context::load_pr_facts` lacks direct same-symbol unit coverage. The
   higher-level status path covers it, and this sweep found no false verdict;
   no bug was filed without a reproducible failure.
4. The CLI and TUI drain views now share persisted state, but there is no
   cross-surface equivalence test. Again, no contradiction was reproduced, so
   this remains evidence for review rather than an implementation spec.

## Decisions for the advisor

This report does not choose either issue:

1. Should external-prose classification become a closed architectural boundary
   (for example, all such decisions must pass through a wrapper/typed result),
   or is the current marker-plus-review contract the accepted boundary? The
   former improves enumerability but is a cross-cutting design change.
2. Should aggregate/detail equivalence be a required contract for all status
   surfaces, or only for surfaces that explicitly claim the same population?
   Applying it universally would turn compact summaries into much larger
   compatibility commitments.

## Conclusion

The pattern is mechanically useful when the inventory predicate is explicit:
enumerate decision-bearing instruments, distinguish their accepted input type,
and require a negative control against the instrument/configuration or its
category-defining documents where that input is meaningful. It is not safely
automatable as a general lint. The strongest reusable checks are local:
base-revision ratchets, mutation/negative-control fixtures, generated-contract
comparisons, and shared state models for aggregate/detail views.

<!-- trace:SPIKE-85 | ai:codex -->
