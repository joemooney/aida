# SPIKE-75 - Anthropic advisor tool as a Claude adapter capability

Date: 2026-07-19
Spec: SPIKE-75
Status: research update and recommendation

<!-- trace:SPIKE-75 | ai:codex -->

## Verdict

Build later, but the trigger changed from "not exposed in Claude CLI" to
"wire it as an opt-in Claude adapter capability after AIDA's vendor execution
adapter has a policy/config seam for per-vendor execution features."

As of the 2026-07-19 check, Claude Code exposes the server-side advisor tool
through `/advisor`, `advisorModel`, and `--advisor`. The underlying API feature
is still described as experimental/beta and Anthropic-API-only, not available on
Bedrock, Claude Platform on AWS, Google Cloud's Agent Platform, or Microsoft
Foundry.

Sources checked:

- Claude Code CLI reference: `--advisor <model>` enables the server-side advisor
  tool for a session.
- Claude Code advisor docs: `/advisor`, `advisorModel`, and `--advisor` are the
  supported enablement surfaces.
- Claude Platform advisor docs: the API tool remains beta under
  `advisor-tool-2026-03-01`; the advisor runs server-side without tools or
  context management.
- Claude Platform release notes: `max_tokens` support for advisor output landed
  on 2026-06-02.

## Layering Decision

Anthropic's "advisor tool" is an inference-time tool inside one Claude
conversation. AIDA's "advisor" is a durable coordination role that can answer
punts, arbitrate strategy, record decisions, and escalate to the human.

Do not merge those concepts.

Use the Anthropic feature only behind the Claude execution adapter:

```text
AIDA coordination layer
  implementer -> reviewer/advisor/human contract stays unchanged

vendor execution adapter
  Claude: may launch with --advisor <model>
  Codex: use its own native equivalent when one exists, else ordinary second pass
  Gemini/other: same adapter-local choice
```

The AIDA-level contract remains vendor-neutral:

- implementer produces work;
- reviewer/advisor evidence is recorded through existing AIDA files/specs;
- escalation decisions remain governed by Type A/B/C, ADR-14, and subsystem
  authority policy.

The Claude advisor tool may improve a Claude implementer's in-turn planning, but
it must not replace AIDA's advisor tier or create invisible approvals.

## Capability Mapping

| AIDA concern | Anthropic advisor tool fit |
|---|---|
| Long-running Claude coding task | Good: main model can consult a stronger model at decision points. |
| AIDA punt to durable advisor role | Partial only: advice is inside the Claude turn, not a substrate verdict. |
| Cross-vendor coordination | No: Claude-only and Anthropic-API-only. |
| Auditability | Weak by default: AIDA must still record decisions in spec comments/findings/PR notes. |
| Cost control | Improved by `max_tokens`, `max_uses`, and model pairing, but adapter-local. |
| Security/governance | Does not loosen AIDA gates; no AIDA write should be authorized solely by this tool. |

## Recommended Build Shape

Add config only when the vendor adapter seam is ready:

```toml
[agents.vendor.claude.advisor_tool]
enabled = false
model = "opus"
max_uses = 2
max_tokens = 2048
contexts = ["guided", "drive"]
```

Operational rules:

- default off;
- allowed only for Claude sessions whose main model supports the advisor;
- never enabled for human-gated/operator-mode work as a substitute for review;
- do not pass it through non-Anthropic gateways unless the adapter confirms
  support;
- surface it in session metadata: `vendor_advisor_tool = claude:opus`;
- require a normal AIDA review/verdict/punt artifact before merge.

## Revisit Trigger

Revisit for implementation when both are true:

1. AIDA's vendor execution adapter has a stable config seam for per-vendor
   launch capabilities.
2. A Claude adapter can pass `--advisor <model>` in a way that records the
   selected advisor model in session/drain metadata without changing AIDA's
   advisor-role routing.

The former "CLI exposure" condition is now satisfied for Claude Code. The
remaining gate is AIDA's adapter/config integration and audit surface.

## Follow-up Acceptance

A future implementation spec should require:

- unit coverage for config parsing and launch-argument rendering;
- dry-run/session-manifest evidence that `--advisor` was selected;
- no change to AIDA advisor-tier routing or punt semantics;
- docs that disambiguate "Claude advisor tool" from "AIDA advisor role";
- a small before/after dogfood run comparing Claude with and without the tool on
  one bounded implementation task.
