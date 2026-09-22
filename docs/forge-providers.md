# Forge providers — GitHub, GitLab, and pure-git

*Last updated: 2026-09-15*

AIDA's PR/MR + CI lifecycle is becoming **forge-agnostic**. The target is that
the same `aida queue work --auto-complete` drain that opens a GitHub PR,
watches CI, merges, and auto-bumps the spec to `Completed` also works against a
GitLab project (where the change request is a *Merge Request*) and against a
plain git remote that has no forge CLI at all. EPIC-35 shipped the
forge-provider abstraction; EPIC-68 is the current home for the remaining
GitLab parity wiring.

The orphan `aida-store` branch was already forge-agnostic (it rides whatever
`origin` is). The forge-provider work extends that toward the *lifecycle*
operations that used to shell out to `gh` unconditionally. The first inventory
described this as roughly 113 direct GitHub sites; the dated gap inventory later
narrowed the runtime wiring gap to roughly 20-25 direct `gh` spawns across
about 18 functions.

---

## The three providers

| Provider | Change-request noun | Forge CLI | "Is it merged?" |
|---|---|---|---|
| **GitHub** | PR | `gh` | open PR → merged PR |
| **GitLab** | MR | `glab` | open MR → merged MR |
| **pure-git** | "change" | *(none)* | git ancestry + `(SPEC-ID)`-trailer auto-complete |

**pure-git** is the default when the origin host is neither GitHub nor GitLab
(or there's no `origin` yet). It works direct-to-default-branch: "merged" is a
`git merge-base --is-ancestor` query, and the spec auto-bumps `Done → Completed`
when a commit carrying its `(SPEC-ID)` trailer lands on the default branch. No
forge CLI is required — any git remote (including a brand-new GitLab project) is
usable immediately, before MR-drain parity is exercised.

---

## Auto-detection at `aida init`

`aida init` inspects `origin`'s host and scaffolds the `[forge]` block in
`.aida/config.toml` accordingly:

- `github.com` (or `*.github.com`) → `provider = "github"`
- a host containing `gitlab` (e.g. `gitlab.com`, self-hosted `gitlab.example.com`)
  → `provider = "gitlab"`
- anything else, or no origin → `provider = "pure-git"`

init prints a one-line confirmation so you see the inference without opening the
config file:

```
Done  Detected forge: GitLab (origin host) — MR + CI lifecycle via `glab` (install: https://gitlab.com/gitlab-org/cli).
```

To override the detection — for example a self-hosted GitLab on an unusual host,
or to force pure-git on a GitHub repo — edit the scaffolded block:

```toml
[forge]
provider = "gitlab"   # github | gitlab | pure-git
```

Accepted tokens are case-insensitive: `github`/`gh`, `gitlab`/`glab`,
`pure-git`/`none`/`git`. Config wins over auto-detection; auto-detection wins
over the pure-git fallback.

---

## MR linkage in `aida show`

`aida show <SPEC>` renders a **Git linkage** section: the commits that reference
the spec, the files carrying its `trace:` comments, the branch/worktree the work
lives on, and the change-request state. That section is forge-aware — on a
GitLab project it reads "MR", not "PR":

```
Git linkage:
  Branch     feature/story-511 (worktree: …) · in flight
  MR         MR-13 https://gitlab.com/joe/aida-gl-test/-/merge_requests/13
  Commits (1)
    a1b2c3d [AI:claude] feat(forge): … (STORY-511)
  Files traced (1)
    aida-cli/src/main.rs — format_change_linkage
```

After the MR merges, the same section reports:

```
Git linkage:
  Branch     merged to main
  MR         MR-13
  …
```

The noun ("PR"/"MR"/"change") and the CLI named in any "state unknown"
diagnostic ("`glab` not installed", "`gh` lookup failed") follow the resolved
forge, so a GitLab user never reads GitHub-only wording. The lookup routes
through the `Forge` provider (`change_for_branch`), which calls `glab mr list`
for GitLab and `gh pr list` for GitHub.

---

## End-to-end GitLab drain

On a GitLab project (`[forge] provider = "gitlab"`, with `glab` installed and
authenticated), the intended full autonomous drain mirrors GitHub:

```bash
aida queue work --auto-complete
```

per-spec lifecycle: implementer → CI → reviewer → **MR opened** (`glab mr
create`) → MR merged (`glab mr merge --squash --remove-source-branch`) → `aida
pull` auto-bumps the spec to `Completed`.

EPIC-35 is marked `Completed`, but that status is historical overclaim: it
landed the `Forge` trait, providers, and several routed paths, while the
remaining GitLab wiring is tracked under EPIC-68. The two relevant GitLab code
bodies are separate: `aida-cli-lib/src/forge.rs` contains `GitLabForge` for the
PR/MR + CI lifecycle, while `aida-core/src/integrations/gitlab/client.rs`
contains `GitLabClient` for the GitLab Issues integration.

**Live-GitLab trait validation runs nightly.** The scheduled
`cross-platform.yml` workflow creates a throwaway project on the self-hosted
mirror, opens an MR through `Forge::open_change`, waits for a real pipeline,
reads its review verdict, and merges it through `Forge::merge_change`. The job
deletes the project even after a test panic. It reports a skipped probe (rather
than a red build) when the mirror is unreachable. A missing
`AIDA_GITLAB_TOKEN` repository secret is a hard configuration failure, as are
all lifecycle failures after a successful preflight; each fails with the
affected phase named.
Ordinary PR CI remains hermetic and continues to test provider parsers with
mocked output. <!-- trace:TASK-1273 | ai:codex -->

#### Cloudflare Access authentication

The GitHub-hosted live-smoke runner crosses two independent authentication
boundaries. `AIDA_GITLAB_TOKEN` authorizes GitLab API operations, while the
dedicated Cloudflare Access service-token pair
`AIDA_CF_ACCESS_CLIENT_ID`/`AIDA_CF_ACCESS_CLIENT_SECRET` admits the request
through the perimeter. All three are GitHub Actions repository secrets. The
Access application for `gitlab.joemooney.com` must have a **Service Auth**
policy that accepts the dedicated CI service token. <!-- trace:TASK-1425
trace:ADR-54 | ai:codex -->

The workflow pins a current `glab` release and configures host-scoped
`custom_headers`. Header values use `valueFromEnv`; secret values must never be
written literally into `config.yml`, committed, or printed in diagnostics.
Git clone, fetch, and push use Git's numbered environment configuration to add
the same two headers only to HTTPS requests for the mirror host. The values
exist only in the live-test process tree and are unset on exit; repository and
global Git configuration remain free of the Access secret. A process-scoped
`glab auth git-credential` helper supplies the separate GitLab PAT.
The API preflight calls `/api/v4/version` with both Access headers and validates
the JSON response. A response beginning with `<!DOCTYPE html>` and titled
`Sign in ・ Cloudflare Access` means the request did not pass the perimeter; a
successful readiness probe alone does not establish GitLab API access.

Rotate the Access pair in Cloudflare and GitHub together: create the replacement
service token, authorize it in the Service Auth policy, replace both repository
secrets, prove a live run, and then revoke the old token. If the credential may
be compromised, revoke it in Cloudflare first and accept that the smoke stays
red until replacement secrets are installed. Rotate or revoke the GitLab PAT
separately; neither credential substitutes for the other.

### Live validation (2026-09-18)

A headless `--auto-complete --no-human=both` drain was run against the
self-hosted mirror at `gitlab.joemooney.com` (project `ai/aida`), with the
scaffolded `merge-hold-gate` job wired into `.gitlab-ci.yml`. The run exercised
CI registration, the CI watch, per-job red refinement, and the merge-hold gate.

### Manual validation checklist (GitLab)

1. `glab auth status` — confirm `glab` is installed and logged in.
   If it is not, run `glab auth login` and authenticate against the target
   GitLab host.
2. Clone/create a GitLab project; `aida init` → confirm
   `Detected forge: GitLab` and `provider = "gitlab"` in `.aida/config.toml`.
3. File a small spec: `aida add --title "…" --status approved`.
4. Drain it: `aida queue work <SPEC> --auto-complete`.
5. Watch for: an MR opened on GitLab, CI run via `glab ci`, the MR merged, and
   the spec auto-bumped to `Completed` on the next `aida pull`.
6. `aida show <SPEC>` → confirm the **Git linkage** section names the MR
   (`MR-NN` + the `/-/merge_requests/NN` URL) and reads "merged to main" after
   the merge.

---

## Related

- `docs/git-verb-surface.md` — the two-leg git-mirror verbs (`fetch`/`pull`/`push`/`rebase`).
- `docs/autonomous-drain.md` — the `--auto-complete` drain lifecycle.
- `docs/plans/2026-06-04-forge-provider.md` — the SPIKE-49 design + the `Forge` trait surface.
- `docs/plans/2026-06-14-epic35-gh-forge-gap-inventory.md` — the dated direct-`gh` wiring inventory.
- `aida-cli-lib/src/forge.rs` — `ForgeKind`, the `Forge` trait, `GitHubForge` / `GitLabForge` / `PureGitForge`.

<!-- trace:STORY-511 trace:EPIC-35 trace:TASK-1242 | ai:claude+codex -->
