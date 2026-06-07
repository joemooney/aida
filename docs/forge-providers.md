# Forge providers — GitHub, GitLab, and pure-git

*Last updated: 2026-06-06*

AIDA's PR/MR + CI lifecycle is **forge-agnostic**. The same `aida queue work
--auto-complete` drain that opens a GitHub PR, watches CI, merges, and
auto-bumps the spec to `Completed` works against a GitLab project (where the
change request is a *Merge Request*) and against a plain git remote that has no
forge CLI at all. This is EPIC-35 — the forge-provider abstraction.

The orphan `aida-store` branch was already forge-agnostic (it rides whatever
`origin` is). EPIC-35 extends that to the *lifecycle* operations that used to
shell out to `gh` unconditionally.

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
authenticated), the full autonomous drain works the same as GitHub:

```bash
aida queue work --auto-complete
```

per-spec lifecycle: implementer → CI → reviewer → **MR opened** (`glab mr
create`) → MR merged (`glab mr merge --squash --remove-source-branch`) → `aida
pull` auto-bumps the spec to `Completed`. CI status comes from `glab ci
status` / `glab ci list` (STORY-510), and the open/status/merge operations from
`glab mr …` (STORY-509).

**Live-GitLab end-to-end validation is a manual step.** The CI here is
Linux-only and has no GitLab credentials, so the e2e drain (MR
opened → CI → reviewed → merged → spec auto-bumped) is validated by hand against
a scratch GitLab project (`joe/aida-gl-test`), not in automated CI. The pure
formatting and the provider parsers are unit-tested in isolation; the
subprocess-level `glab` wiring is exercised manually.

### Manual validation checklist (GitLab)

1. `glab auth status` — confirm `glab` is installed and logged in.
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
- `aida-cli/src/forge.rs` — `ForgeKind`, the `Forge` trait, `GitHubForge` / `GitLabForge` / `PureGitForge`.

<!-- trace:STORY-511 trace:EPIC-35 | ai:claude -->
