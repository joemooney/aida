// AIDA SPIKE-32 narrow POC: saved-script workflow lane for one shipped spec.
//
// Target spec: SPIKE-30
// Intent: prove the happy-path shape for "spec graph -> workflow.js artifact
// -> Claude Code workflow runtime replay -> AIDA supervisor observes".
//
// API status:
// - Claude's public workflows docs confirm saved project workflows live under
//   `.claude/workflows/*.js`, run as slash commands, coordinate subagents, keep
//   intermediate results in script variables, and cannot directly access shell
//   or filesystem.
// - The public docs do not publish the full JS helper signature. The installed
//   math-olympiad workflow guidance says to set `opts.label` on every
//   `agent()` call. This POC therefore uses `agent(prompt, { label })`.
// - When run with plain Node, the script uses a mock `agent` fallback so the
//   phase/result contract is executable and reviewable without Claude Code.
//
// trace:SPIKE-32 | ai:codex

const SPEC = {
  id: "SPIKE-30",
  title: "Integrate `claude agents --json` into `aida status`",
  status: "Completed",
  priority: "High",
  description: [
    "Query `claude agents --json` and cross-reference against AIDA's lease",
    "registry (`.aida/sessions/`). Surface unified state in `aida status`.",
    "Detect and surface drift: leases without processes, processes without",
    "leases. Operator gains single-command visibility into running Claude",
    "Code work plus AIDA lifecycle state.",
  ].join(" "),
  acceptance: [
    "AIDA status invokes or ingests `claude agents --json`.",
    "Output schema includes pid, cwd, kind, startedAt, sessionId, name, status.",
    "AIDA leases are reconciled against Claude Code process state.",
    "Status output surfaces matched agents plus drift in either direction.",
  ],
  shippedEvidence: {
    commit:
      "dc9b7bf2 [AI:claude] feat(status): integrate claude agents --json — cross-substrate view (SPIKE-30)",
    tracedFiles: [
      "aida-cli/src/claude_agents.rs",
      "aida-cli/src/main.rs — print_status_claude_code_section",
    ],
  },
};

const REVIEW_APPROVE_RE = /\b(approve|approved|pass|passed|ship|looks good)\b/i;

async function callAgent(runtime, phase, prompt) {
  const startedAt = new Date().toISOString();
  const result = await runtime.agent(prompt, {
    label: `${SPEC.id}:${phase}`,
  });
  return {
    phase,
    ok: true,
    startedAt,
    completedAt: new Date().toISOString(),
    result,
  };
}

async function waitForCiStub() {
  return {
    phase: "ci",
    ok: true,
    stub: true,
    result:
      "CI wait is stubbed in this POC. A production workflow would either ask an agent to poll `gh pr checks` or let the AIDA supervisor observe CI externally.",
  };
}

function implementationPrompt() {
  return `
You are the AIDA implementer for ${SPEC.id}: ${SPEC.title}

This is a POC replay against an already-shipped spec. Do not edit files.

Spec description:
${SPEC.description}

Acceptance criteria:
${SPEC.acceptance.map((item) => `- ${item}`).join("\n")}

Shipped evidence:
- Commit: ${SPEC.shippedEvidence.commit}
${SPEC.shippedEvidence.tracedFiles.map((item) => `- File: ${item}`).join("\n")}

Task:
Return a concise JSON-like implementation summary:
- spec_id
- intended code touchpoints
- implementation_plan
- expected_pr_subject
- whether this happy-path phase could be generated from the spec graph
`;
}

function reviewPrompt(implementation) {
  return `
You are the AIDA reviewer for ${SPEC.id}: ${SPEC.title}

This is a POC replay against an already-shipped spec. Do not edit files.

Acceptance criteria:
${SPEC.acceptance.map((item) => `- ${item}`).join("\n")}

Implementation phase output:
${stringifyForPrompt(implementation)}

Shipped evidence:
- Commit: ${SPEC.shippedEvidence.commit}
${SPEC.shippedEvidence.tracedFiles.map((item) => `- File: ${item}`).join("\n")}

Task:
Return a reviewer verdict with:
- verdict: Approve | RequestChanges
- acceptance_check
- risk_notes
- whether reviewer routing is expressible in workflow.js happy path
`;
}

function mergePrompt(review) {
  return `
You are the AIDA merger for ${SPEC.id}: ${SPEC.title}

This is a POC replay against an already-shipped spec. Do not merge anything.

Reviewer output:
${stringifyForPrompt(review)}

Task:
Return the merge plan that a real AIDA supervisor would execute:
- confirm reviewer approved
- confirm CI green
- confirm squash subject includes (${SPEC.id})
- confirm post-merge auto-bump would mark the spec Completed
`;
}

function stringifyForPrompt(value) {
  return JSON.stringify(value, null, 2);
}

async function runWorkflow(runtime = defaultRuntime()) {
  const phases = [];

  const implementer = await callAgent(runtime, "implementer", implementationPrompt());
  phases.push(implementer);

  const ci = await waitForCiStub();
  phases.push(ci);

  const reviewer = await callAgent(runtime, "reviewer", reviewPrompt(implementer.result));
  phases.push(reviewer);

  const approved = REVIEW_APPROVE_RE.test(stringifyForPrompt(reviewer.result));
  if (!approved) {
    return {
      spec_id: SPEC.id,
      status: "stopped",
      reason: "reviewer did not approve",
      phases,
      failure_routing_probe:
        "Shelve/NeedsAttention would need an AIDA CLI callback or supervisor action here.",
    };
  }

  const merger = await callAgent(runtime, "merger", mergePrompt(reviewer.result));
  phases.push(merger);

  return {
    spec_id: SPEC.id,
    status: "happy_path_complete",
    phases,
    structured_result: {
      implementer: summarizePhase(implementer),
      ci: summarizePhase(ci),
      reviewer: summarizePhase(reviewer),
      merger: summarizePhase(merger),
    },
    failure_routing_probe: {
      punt:
        "Expressible only if implementer returns a structured punt result. Rerouting to advisor is easy; resuming same implementer context is not proven.",
      shelve:
        "Branching on RequestChanges is easy. Marking NeedsAttention requires an AIDA CLI/MCP callback outside the pure script.",
      resume:
        "Awkward. Claude workflow resume is run-local; AIDA punt resolution is substrate-local and may happen after the workflow stops.",
    },
  };
}

function summarizePhase(phase) {
  return {
    ok: phase.ok,
    startedAt: phase.startedAt,
    completedAt: phase.completedAt,
    stub: phase.stub === true,
  };
}

function defaultRuntime() {
  if (typeof globalThis.agent === "function") {
    return {
      agent: globalThis.agent,
    };
  }

  // Local validation fallback: proves the happy-path data contract without
  // requiring Claude Code's workflow runtime. This branch is not the target
  // runtime; it exists so `node spec-30-drain.workflow.js` is useful.
  return {
    async agent(prompt, opts = {}) {
      return {
        label: opts.label,
        prompt_excerpt: prompt.trim().slice(0, 360),
        verdict: opts.label?.endsWith(":reviewer") ? "Approve" : "Complete",
        note: "mock agent result; replace with Claude Code workflow runtime agent()",
      };
    },
  };
}

if (typeof module !== "undefined" && require.main === module) {
  runWorkflow()
    .then((result) => {
      console.log(JSON.stringify(result, null, 2));
    })
    .catch((error) => {
      console.error(error);
      process.exitCode = 1;
    });
}

if (typeof module !== "undefined") {
  module.exports = {
    SPEC,
    runWorkflow,
  };
}
