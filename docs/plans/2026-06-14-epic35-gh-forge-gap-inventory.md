# EPIC-35 forge wiring-gap inventory — `gh` call sites in `main.rs` (2026-06-14)

**Specs:** TASK-820 · **Status:** inventory (dated snapshot) · **Complexity:** audit feeding migration planning
**Provenance:** delegated 6-way parallel audit of `aida-cli/src/main.rs` + synthesis, **spot-checked against code** by the advisor (see §0). Treat counts as audit-grade, not line-perfect.

> Frozen at time T. Supersede with a new dated file; do not retro-edit.

## 0. Why this exists + provenance

Tier-2 of the GitLab primary-repo evaluation (Tier-1 proved the git-canonical *store* round-trips on self-hosted GitLab — see BUG-559). This inventory maps the *forge* layer: which `gh` call sites in `main.rs` still bypass the `Forge` trait and so break on GitLab.

**EPIC-35 is marked Completed, but it isn't.** EPIC-35 ("GitLab as a first-class forge") shipped the *abstraction* — `ForgeKind {GitHub, GitLab, PureGit}`, the `Forge` trait, `GitHubForge`/`GitLabForge`/`PureGitForge`, `forge_for()` — but `forge.rs`'s own header defers the call-site migration to "follow-on commits," and those never finished. The epic's Completed status overclaims the real state ("plumbed, not wired"). The remaining wiring needs a **new** spec home, not a child of a closed epic.

**Spot-checks (advisor-verified against code, 2026-06-14):**
- `resolve_glab_binary` — **confirmed absent** (Slice 0 foundation is real).
- 106412 / 106455 — **confirmed** raw `Command::new("gh") … ["pr","create","--fill"]` in both `queue recover` arms.
- 112134 — **confirmed** `gh api repos/:owner/:repo/commits/<sha>/check-runs` (delegated-review poll).
- `Forge::merge_change` — **confirmed** exists (trait + 3 impls) → 76317 is route-only.

## 1. Executive summary

**158 classified sites total**, but most are noise (doc comments, error-string wording, test fixtures, paste-ready hint text). The honest gap:

| Bucket | Count | Meaning |
|---|---|---|
| `direct_gh` (builds gh argv directly) | **24** | Spawns gh, bypasses Forge. ~2 are printed strings, not spawns. |
| `gh_helper` (via `resolve_gh_binary`/`gh_pr_list_first`, not the trait) | **5** | Spawns gh through the central choke point but bypasses the trait. |
| `forge_trait` (already routed) | **17** | Migration-complete; only error wording is gh-flavored. |
| `test_or_comment` (not runtime) | **112** | Comments, labels, hints, test fixtures — wording polish, not wiring. |

**The real wiring gap is ~20–25 runtime spawns across ~18 functions**, gated behind one foundation. The "~113 gh sites" framing overstates the work by ~4–5×.

## 2. Gap breakdown by operation category (runtime spawns only)

| Category | Gap count | gitlab_ready | glab equivalent |
|---|---|---|---|
| `ci_status_watch` | 6 (47555, 47579, 42110, 42298, 76305, 112134) | no | `glab ci status` / `glab ci list -F json` / `glab ci view --live` / `glab api …/statuses` |
| `pr_create` | 2 (106412, 106455) | partial | `glab mr create --fill` |
| `pr_merge` | 1 (76317; release `--after-pr`) | yes | `glab mr merge <N> --squash --remove-source-branch` |
| `pr_view_list_status` | 12 (44618, 44666, 44749, 46455, 46598, 47420, 47666, 47910, 48058, 67137, 94760, 94950) | mostly no/partial | `glab mr view <n> -F json` / `glab mr list --source-branch` / `glab mr diff` |
| `run_view`/`workflow` | 1 (87801, x-platform gate) | no | `glab ci list --branch` (no `--workflow`; GH-Actions-specific) |
| `api_generic` | 0 spawns (only rate-limit hint strings) | no | `glab api …` |

**Printed-string / generated-artifact gaps** (GitHub-worded surfaces, not spawns): 69312 (`gh pr merge` in the queue "Done — awaiting merge" section), 70689 (`gh pr view --json state` baked into the `/goal --pr` clause), plus ~30 `.cyan()` paste-ready hints.

## 3. Priority order (GitLab user's lifecycle order)

1. **`pr_create` ★** — 106412, 106455 (`queue recover` raw spawns; the `pr ship` create path is already routed). First breakage on parked-spec recovery.
2. **`ci_status_watch` ★** — biggest cluster: 47555/47579 (`wait_for_pr_checks_to_register`), 42110 (`probe_ci_state_for_branch`), 42298 (`watch_ci_terminal`), 112134 (delegated-review poll), 76305 (release). `--auto-complete` hits CI-watch early; gh-output-shape coupling breaks it.
3. **`pr_merge` ★** — mostly routed (46871, 112436 via `merge_change`); residual 76317 is release-only.
4. **`pr_view_list_status` ★** — largest count. Early-hit: 46598 (`pr ship` lookup), 47910/48058 (reviewer pre-flight), 94760/94950 (`aida review`), 44618/44666/44749 (merged-PR reconcile). Later: 46455, 67137, 47420.
5. **`api_generic`** — cosmetic hint strings only.
6. **`run_view`/`workflow`** — 87801 + `gh workflow run` hint; GH-Actions-specific, release-only, may stay GH-guarded.

## 4. GitLabForge coverage: route-only vs new-method

**Route-only (trait method already exists, just point the site at it):** `pr_merge` (`merge_change`); `pr_create` recover arms (`open_change`); open-PR-for-branch 46598/47666 (`change_for_branch`); PR diff 94760 + base/head 94950 (consolidate the hand-rolled `ReviewForge` switch onto the trait).

**Needs NEW GitLabForge methods:** CI check-registration + terminal watch (47555/47579, 42110, 42298) — output-shape-decoupled; delegated-review check-runs poll 112134 (forge-abstracted severity tally — the deepest gap, GitLab `…/statuses` differs in shape); arbitrary PR-metadata read `change_metadata(number, fields)` (44666/44749/46455/47420/67137); reviewer pre-flight 47910/48058 (built on the metadata method); draft toggle 46764 (`set_draft`); cross-platform CI run-list 87801 (or accept GH-only with a guard).

## 5. Recommended slicing (smallest-valuable-first)

**Slice 0 — `resolve_glab_binary()` + forge-keyed binary dispatch (foundation).** Sibling resolver; make the central helpers (`resolve_gh_binary`, `gh_pr_list_first`, `fetch_pr_info_via_gh_bin`) forge-aware. No GitHub behavior change. Unblocks every slice below. **Build once first.**

**Slice 1 — route-only PR create + merge + open-PR-for-branch (≈5 sites).** 106412, 106455 → `open_change`; 76317 → `merge_change`; 46598, 47666 → `change_for_branch`. Highest value / lowest effort.

**Slice 2 — PR-metadata read method + view/list consolidation (≈9 sites).** Add `Forge::change_metadata`; route 44618/44666/44749 (drain reconcile), 46455, 47420, 47910, 48058 (reviewer pre-flight), 94950, 67137. Makes the drain's reconcile + reviewer phases GitLab-safe.

**Slice 3 — CI status/watch abstraction (≈6 sites, deepest).** New `wait_for_checks_to_register` / `watch_ci_terminal` / output-decoupled `ci_probe_for_branch`; route 47555/47579, 42110, 42298, 76305, and 112134 (own forge-abstracted severity tally). Unblocks unattended `--auto-complete` on GitLab. Do last (most output-shape-coupled).

**Slice 4 — user-facing text + generated artifacts (cosmetic, ~40 strings).** Derive all hint/`/goal`-clause wording from `ForgeKind::cli_name()`/`change_noun()` so a GitLab user never reads "gh pr merge". No spawns; pure polish; can trail.

## 6. Top 10 highest-priority individual sites

| # | Line | Function | gh command | Why it matters |
|---|---|---|---|---|
| 1 | 112134 | `run_reviewer` | `gh api …/check-runs` | Drain's delegated-review phase; no Forge method; `--auto-complete` + delegated review **fails** on GitLab. Deepest gap. |
| 2 | 47579 | `wait_for_pr_checks_to_register_with_gh` | `gh pr checks` (poll) | Runs before every CI watch; output-shape-coupled. |
| 3 | 47555 | `wait_for_pr_checks_to_register` | hardcodes `gh` | Entry to #2; the binary-name hardcode. |
| 4 | 46598 | `pr ship` (step 1) | `gh pr list --head --json number` | Open-PR lookup; `change_for_branch` exists — pure routing miss. |
| 5 | 106412 | `handle_queue_recover` (PushOpenPrDrive) | `gh pr create --fill` | Recover opens a PR via raw gh; GitLab recovery breaks. |
| 6 | 106455 | `handle_queue_recover` (WipCommitPushDrive) | `gh pr create --fill` | Same in the WIP arm. |
| 7 | 47910 | `preflight_stale_base_check` | `gh pr view` | Reviewer pre-flight in the drain; early-hit per reviewed spec. |
| 8 | 48058 | `preflight_intermediate_only_check` | `gh pr view` | Reviewer pre-flight; intermediate-only classification. |
| 9 | 94760 | `handle_review_command` | `gh pr diff` | `aida review` is frequent; `gitlab_ready: yes` — route-only. |
| 10 | 42110 | `probe_ci_state_for_branch` | `gh pr list --json statusCheckRollup` | CI-state probe; not routed through `ci_probe_for_branch`. |

## 7. Bottom line

The honest runtime gap is **~20–25 spawns across ~18 functions**, gated behind one foundation (`resolve_glab_binary()` + forge-keyed dispatch). Slices 1–2 (route-only PR create/merge/view, ~14 sites) make the keyboard paths GitLab-safe cheaply; Slice 3 (CI watch + the delegated-review `gh api` poll) is the hard, output-shape-coupled remainder that unblocks unattended `--auto-complete` on GitLab. The other ~112 sites are wording/test polish. EPIC-35's Completed status should be corrected and the wiring tracked under a new spec.

## Related

- BUG-559 (GitLab default-branch / fresh-clone auto-attach — the Tier-1 substrate bug)
- `aida-cli/src/forge.rs` (the `Forge` trait + providers this routes through)
- EPIC-35 (Completed prematurely — abstraction shipped, wiring deferred)
