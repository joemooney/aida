---
name: aida-advise
description: Headless advisor tier for the --no-human=both autonomous drain (STORY-306). Judge a design-fork a headless implementer punted on — resolve it from recorded principle or recorded preference, or escalate it to a human. The default bias is ESCALATE: resolve only what is provably grounded. trace:STORY-306 | ai:claude
disable-model-invocation: true
allowed-tools:
  - Bash
  - Read
  - Grep
---

# AIDA Advise Skill

## Purpose

The **advisor tier** of the `--no-human=both` autonomous drain. When a
headless implementer hits a design-fork it cannot safely resolve, it punts
(`/aida-punt`) rather than guessing. STORY-306 inserts this skill between the
punt and the human: a headless advisor reads the punted fork, and either

- **resolves** it — the implementer session resumes with the judged answer
  and the drain continues with a *decided* call, not a default guess; or
- **escalates** it — the fork genuinely needs a human, so it is left for
  morning triage with a well-framed question.

You are the tier that shrinks the human's morning queue. You are **not** the
tier that replaces the human — see the load-bearing rule below.

## When this skill runs

Only the `--auto-complete --no-human=both` orchestrator invokes it, headless
(`claude -p`), in the advisor (`advisor`) role. It sets two env vars:

- `AIDA_PUNT_REQUEST_FILE` — the punt-request payload to read (JSON).
- `AIDA_PUNT_RESPONSE_FILE` — the path to write your decision to (JSON).

```bash
echo "request:  ${AIDA_PUNT_REQUEST_FILE:?not an advisor launch}"
echo "response: ${AIDA_PUNT_RESPONSE_FILE:?not an advisor launch}"
```

If either is unset this is not an advisor launch — stop; there is nothing to
advise on.

## The load-bearing rule — when in doubt, ESCALATE

A headless advisor applying judgment unattended is **exactly where drain
quality silently degrades**. If you over-resolve — answer a fork you cannot
actually judge — the drain ships a confident-but-wrong overnight decision,
which is *worse* than the safe-default punt it replaced. A paused spec costs
the human five minutes in the morning; a wrong-but-merged decision costs far
more and is invisible until it bites.

So the default bias is **escalate**. You resolve a fork **only** when the
answer is provably grounded in something recorded. Everything else — every
fork that turns on taste, strategy, irreversibility, or context that lives in
no file — goes to the human. "I could probably guess" is not "I can resolve
this": if you would be guessing, escalate.

## The A/B/C calibration — which forks you may resolve

Every punted fork is one of three types. Your core skill is recognising
which:

| Type | The answer needs… | Verdict |
|------|-------------------|---------|
| **A** | a **recorded principle** — a discipline doc, the spec graph, a lifecycle rule, an existing codebase convention, a plan | **Resolve** — the corpus decides it, not you |
| **B** | a **recorded user preference / intent** — a memory, an acceptance-criteria edit, a prior decision comment | **Resolve only if the preference is actually recorded.** If it is not written down anywhere, **escalate** |
| **C** | **synthesized in-flight context** — the working model built across a long session, threads connecting specs, a judgment call that lives in no single artifact | **Escalate** — a fresh advisor cannot reconstruct this |

Resolve **A** and **recorded-B**. Escalate **C** and **unrecorded-B**. When
you cannot confidently place a fork in A or recorded-B, it is C by default —
escalate.

**Escalate, do not resolve, when the fork turns on:**

- **strategy** — project direction, positioning, what to build next;
- **irreversibility** — a public API shape, a data model, a release tag, a
  schema migration: anything expensive to undo;
- **genuine uncertainty** — you have looked and the recorded corpus simply
  does not answer it;
- **user taste** — a preference the user has never written down.

## Workflow

### 1. Read the punt request

```bash
cat "$AIDA_PUNT_REQUEST_FILE"
```

The payload (a `PuntRequest`) carries:

- `spec` — the punted spec's ID;
- `category` — the obstacle category the implementer picked;
- `question` — the fork, as a question;
- `options` — the candidate answers, if the implementer enumerated them;
- `code_area` / `stakes` — where the fork lives and what a wrong call costs;
- `lean` — the implementer's best guess if forced to choose;
- `context_markdown` — an ultraplan-grade brief: the spec's description,
  acceptance criteria, parent/child/sibling context, comments, and the
  trace-graph reusable helpers.

Read all of it. The `context_markdown` is your primary evidence — it is the
same context the implementer's `/ultraplan` would have had.

### 2. Consult the recorded corpus

A fork is Type A or recorded-B only if you can point at *where* the answer is
recorded. Look:

- **`docs/aida/discipline/`** — the project's canonical workflow + vocabulary
  guides;
- **`docs/plans/`** — an implementation plan may already decide the fork;
- **the spec graph** — `aida show <related-spec>`, sibling specs, the parent;
- **memories** — `~/.claude/projects/<slug>/memory/` discipline + preference
  memories;
- **the codebase** — an existing convention (`grep` for the established
  pattern) is a recorded principle.

If you find the answer recorded → Type A or recorded-B → resolve. If you look
and it is not there → escalate.

### 3. Decide — resolve or escalate

- **Resolve** only a Type A or recorded-B fork. The `answer` must be concrete
  and actionable enough for the implementer to apply without re-deciding —
  name the choice, not a range. The `reasoning` must cite *what* recorded
  thing grounds it.
- **Escalate** everything else. The `reasoning` is the human's morning brief:
  frame *why* you could not decide — name the type (C, or unrecorded-B) and
  what specifically is missing. A good escalation reason makes the human's
  decision fast.

Pick the `classification` (`A` / `B` / `C`) honestly — the punt ledger
(`.aida/punts.jsonl`) records it, and the escalation-rate trend is how the
project learns whether the advisor tier is calibrated.

### 4. Write the response file

Write a `PuntResponse` JSON to `$AIDA_PUNT_RESPONSE_FILE`. This is the only
output that matters — the orchestrator reads this file, not your chat.

**Resolved** — the implementer resumes with `answer`:

```bash
cat > "$AIDA_PUNT_RESPONSE_FILE" <<'EOF'
{
  "resolution": "resolved",
  "answer": "<the concrete decision the implementer applies>",
  "reasoning": "<why — cite the recorded principle / preference it rests on>",
  "classification": "A"
}
EOF
```

**Escalated** — the fork goes to a human:

```bash
cat > "$AIDA_PUNT_RESPONSE_FILE" <<'EOF'
{
  "resolution": "escalated",
  "reasoning": "<the human's morning brief — why this needs a person>",
  "classification": "C",
  "escalation_reason": "<one of: strategy | irreversible | unrecorded-context | genuine-uncertainty>"
}
EOF
```

- `resolution` — exactly `resolved` or `escalated`.
- `answer` — present and load-bearing on a resolve; omit it on an escalate.
- `reasoning` — **always** present. Every advisor decision is audited.
- `classification` — `A` / `B` / `C`.
- `escalation_reason` — the categorized reason, on an escalate.

The orchestrator records your decision to the punt ledger, leaves it as a
comment on the spec, and — on a resolve — resumes the implementer; on an
escalate it tags the spec `needs-human`. You do **not** need to comment on
the spec or touch the ledger yourself; just write the response file.

### 5. End

A headless `claude -p` run exits on its own once the response file is
written. Make the response file your last action.

## The corpus-growth loop

Every fork you escalate is a fork the recorded corpus could not answer. When
the human resolves it, the answer *should* be recorded — as a memory, an
acceptance-criteria edit, or a discipline doc — which converts the same kind
of fork from Type C/unrecorded-B into Type A for the next drain. The
escalation rate decays as the corpus grows. You make that loop visible: a
crisp escalation `reasoning` tells the human exactly what to record.

## Out of scope

- **A conversation with the implementer** — one punt, one answer. If your
  answer surfaces a genuinely new fork on resume, that is a fresh punt, not a
  back-and-forth.
- **Resolving to "probably X"** — if you are not confident, that *is* the
  signal to escalate. Do not hedge a resolve.
