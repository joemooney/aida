# Triage

Two modes — clear the draft inbox, or investigate a bug. Follow the workflow in `.claude/skills/aida-triage.md` and pick the mode by what was asked.

## Inbox triage (clear the draft backlog)

Use when asked to clear/groom the inbox, or when `aida status` shows `Inbox: N untriaged drafts`. **Run as the advisor** (approving + queuing need advisor authority).

1. Size it: `aida list --status draft`
2. For each draft (`aida show <ID>`), pick one disposition:
   - **Keep** → `aida edit <ID> --status approved` + `aida queue add <ID> --for implementer`
   - **Backlog** → leave approved-unqueued, or `aida archive <ID>`
   - **Unclear** → `aida edit <ID> --status needs-attention` (or `--status rejected` to drop)
3. Confirm clear: `aida list --status draft`

## Bug investigation

Use when a bug has been filed and needs a structured investigation.

1. Read the bug requirement (`aida show <BUG-ID>`) and confirm reproduction steps
2. Narrow the failure surface — bisect, log, or add probes as needed
3. Identify root cause with file:line evidence
4. Assess impact (severity, blast radius, data risk)
5. Propose a fix strategy and record findings as comments on the requirement
6. Update status (in-progress, blocked, or ready-for-fix) before exiting
