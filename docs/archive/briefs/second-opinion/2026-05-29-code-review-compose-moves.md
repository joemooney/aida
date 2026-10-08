# Second-opinion brief: AIDA's Code-Review compose strategy

**Date filed**: 2026-05-29
**Target reader**: any independent agent (Codex, Antigravity, web Claude, fresh Claude Code session)
**What's being requested**: stress-test the proposed compose moves for Claude Code's managed Code Review service vs AIDA's existing reviewer phase, find the failure modes I missed, sanity-check the priorities.
**Time budget**: 30–45 minutes. Punchy verdict + the 3 strongest concerns + recommendation.

---

## Context

AIDA is a spec-graph orchestrator. Its 6-phase drain runs: implementer → CI → **reviewer** → merge → pull → build. Phase 3 (reviewer) today:

- Reads PR diff via `gh pr diff`
- Reads the spec's description + acceptance via AIDA MCP
- Spawns a Claude Code session that judges the diff against acceptance criteria
- Posts a structured verdict (Approve / RequestChanges / Comment) via `gh pr comment` + the gh PR API
- The orchestrator parses the verdict and decides: merge, shelve, or escalate

On 2026-05-29 I researched three new Anthropic surfaces I hadn't previously catalogued:

### 1. Managed Code Review (`/en/code-review`)

- Team/Enterprise GitHub App
- Multi-agent review on Anthropic infra ($15-25 per review)
- Posts inline findings tagged 🔴 Important / 🟡 Nit / 🟣 Pre-existing
- Reads `CLAUDE.md` (low priority) AND `REVIEW.md` (highest priority — reviewer-only)
- **Machine-readable severity output:** last line of check-run details is `bughunter-severity: {"normal":N,"nit":M,"pre_existing":K}` — parseable by `gh api ... | jq`
- Manual triggers: `@claude review` (subscribes to push reviews) or `@claude review once` (one-shot)
- Local equivalent: `/code-review [--comment] [--fix] [path|PR]` in any Claude Code session

### 2. GitHub Actions (`anthropics/claude-code-action@v1`)

- General Claude prompt automation in CI
- `@claude` mentions trigger interactive mode; arbitrary events trigger automation mode
- Supports skills + plugin marketplace installation
- Multi-cloud (Anthropic API, Bedrock, Vertex via OIDC)
- Inputs: `prompt`, `claude_args`, `plugins`, `plugin_marketplaces`

### 3. GitLab CI/CD (beta, GitLab-maintained)

- Same pattern as Actions but for GitLab
- Uses `gitlab-mcp-server` for forge actions
- WIF for cloud auth

## SPIKEs filed (the compose moves I'm proposing)

| # | Title | Priority | What it does |
|---|---|---|---|
| 35 | Emit REVIEW.md from spec graph | High | Like SPIKE-31's path-gated rules but for reviewer behavior. AIDA generates REVIEW.md per-PR or per-spec with acceptance criteria, severity calibration, and skip-rules from the spec graph. Code Review reads REVIEW.md as highest-priority instruction. |
| 36 | Parse `bughunter-severity` as orchestrator phase-3 gate | High | AIDA orchestrator phase 3 stops spawning its own reviewer Claude session, parses Code Review's check-run severity tally instead. `normal>0` → RequestChanges, `normal==0` → Approve. Delegates the actual review work. |
| 37 | Trigger Code Review via `@claude review once` from `/aida-review` | Medium | AIDA's reviewer skill comments `@claude review once` on the PR; SPIKE-36 then parses the verdict. Doesn't replace SPIKE-36 — combines with it. |
| 38 | Publish `aida-review` GitHub Action wrapping `claude-code-action@v1` | Medium | Distribution surface: other AIDA-using projects can compose AIDA's discipline pack + reviewer behavior into their CI without local install. |
| 39 | Abstract forge integration (gh vs glab) | Low | Today AIDA's orchestrator is `gh`-coupled. Lower-priority because no near-term GitLab users, but Claude Code's GitLab pattern shows the right abstraction. |

## What I want second-opinion on

### A. Is delegation actually the right move?

**My claim:** Code Review has a fleet of specialized agents + a verification step + millions of dollars of training. AIDA's spec-grounded reviewer is one Claude session reading one prompt. Code Review is strictly more capable on the generic "is this code correct" dimension. AIDA's value-add is acceptance-criteria grounding (REVIEW.md injection) + lifecycle integration (parsing the verdict + acting on it). Delegation makes sense.

**Possible counter:** delegation puts the reviewer outside AIDA's substrate. Spec-graph context only flows in via REVIEW.md. If AIDA's substrate is the moat, putting the reviewer OUTSIDE the moat moves the value where Anthropic captures it, not AIDA. Better to keep AIDA's own reviewer that reads MCP and write to MCP.

**Question:** which framing is right? Is REVIEW.md injection enough substrate flow, or are we leaking the moat?

### B. ZDR / non-Team-tier holdout

Code Review is Team/Enterprise only and NOT available for ZDR (Zero Data Retention) orgs. If AIDA targets ZDR users (security/government adjacent), SPIKE-36's gating logic must have a fallback: when Code Review isn't installed, AIDA's existing reviewer phase runs as today.

**Question:** is this fallback complexity acceptable? Should AIDA's reviewer become two-mode (native vs delegated) permanently, or is "native" the long-tail rename for "ZDR mode"?

### C. Cost shape

Code Review is $15-25 per review. AIDA's existing reviewer phase runs on operator-owned Claude usage (subscription credits, not per-review billable). Delegation shifts cost shape:

- Pre-delegation: AIDA-using project's existing Claude bill, no surprise charges
- Post-delegation: per-PR Code Review charges that the operator might not have budgeted for

**Question:** is this a deal-breaker? Should SPIKE-37 (`@claude review once` trigger) be an opt-in flag per-PR rather than the default?

### D. Where does SPIKE-35 (REVIEW.md emit) actually fire?

Options:
1. **Per-PR generation in CI** — a GitHub Action runs `aida rules sync --review-md` on each PR sync, writes REVIEW.md, Code Review reads it on next review. Tight integration but spreads AIDA's reach into CI.
2. **Per-spec generation locally** — `aida rules sync` writes REVIEW.md for InProgress specs at the implementer's terminal. Commits go into the PR. Code Review reads it via the committed file.
3. **One root REVIEW.md, regenerated on `aida pull`** — covers the whole repo's active scope. Less precise but simpler.

**Question:** which option is right? The trade-off is integration intimacy (option 1 highest, option 3 lowest) vs operator complexity.

### E. The Action vs local-command angle

`/code-review` exists in two flavors:
- The managed Code Review service (cloud, multi-agent, $15-25/PR)
- The local `/code-review` command (runs in any Claude Code session, `--comment` posts to PR, `--fix` patches working tree)

Should AIDA's `/aida-review` skill BECOME a wrapper around `/code-review --comment`? That'd let AIDA users get Anthropic's review quality without the managed service cost, while AIDA injects spec-grounded prompts. SPIKEs 35-37 are all about the managed service; what's the play for the local command surface?

## What I do NOT want second-opinion on

- Whether AIDA's orchestrator's phase 3 (reviewer) is a real phase — it is, it's been shipped for months
- Whether AIDA should COMPETE with Code Review by building our own multi-agent reviewer — that's been decided no, the multi-agent fleet is Anthropic's moat
- Whether REVIEW.md is real — it is, per <https://code.claude.com/docs/en/code-review#review-md>

## Files / surfaces to read for grounding

- `aida-cli/src/auto_complete.rs` — orchestrator state machine, especially phase 3
- `aida-cli/src/reviewer_summary.rs` — current reviewer verdict parsing
- `.claude/skills/aida-review.md` — current reviewer skill
- `aida list --tags reviewer` — every reviewer-relevant spec in the substrate
- Anthropic docs at <https://code.claude.com/docs/en/code-review>

## Desired return shape

Please reply with:

1. **Verdict (1 paragraph):** is the delegation play sound, or are we moving the moat outside AIDA?
2. **Top 3 concerns** with the proposed SPIKE set (35/36/37/38). For each: failure mode + a test we could run to discover it cheaply.
3. **Priority ordering critique:** are 35/36 right as the highest-priority pair? Or is 37 (`@claude review once` trigger) actually the load-bearing first move that the other two compose with?
4. **The ZDR-mode question:** native fallback acceptable? Or kill the delegation play because ZDR users would have a worse experience?
5. **Recommendation:** ship the set as scoped, reshape it, or kill it for a different play.

Under 700 words. Markdown reply.

---

*This brief was generated by AIDA's master advisor session. trace:SPIKE-35 trace:SPIKE-36 trace:SPIKE-37 trace:SPIKE-38 trace:from-strategic-recompose-round-2*
