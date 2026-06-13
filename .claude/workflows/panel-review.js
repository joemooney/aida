export const meta = {
  name: 'panel-review',
  description:
    'Repeatable adversarial multi-agent review. N diverse analysts (blind to each other) → a claims ledger → forced-reproduction skeptics (refute + re-run, not nod) → a reconciled report with PER-CLAIM provenance. Codifies the ECC/SPIKE-50 loop (STORY-518). Args: {target, question, lenses?, maxVerify?, crossVendor?}.',
  phases: [
    { title: 'Analyze', detail: 'N independent analysts, distinct lenses, blind to each other' },
    { title: 'Verify', detail: 'adversarial reproduction — one skeptic per load-bearing claim, must re-run the check' },
    { title: 'Reconcile', detail: 'synthesize report with per-claim provenance + a "what did we all miss?" pass' },
  ],
}

// ---------------------------------------------------------------------------
// Args — target is REQUIRED. No placeholder fallback: passing a placeholder to
// analysts made them silently self-resolve onto different subjects (silent
// subject fork), so an unresolved target must fail before any agent spawns.
// trace:BUG-509 | ai:claude
// ---------------------------------------------------------------------------
const target = typeof (args && args.target) === 'string' ? args.target.trim() : ''
const question = ((args && args.question) || 'Analyze it and compare against the relevant alternative.').toString().trim()
const maxVerify = (args && args.maxVerify) || 12
const crossVendor = !!(args && args.crossVendor)

// Fail fast BEFORE any agent() call: refuse to fan out against an unresolved
// or placeholder subject. trace:BUG-509 | ai:claude
const KNOWN_PLACEHOLDERS = ['the subject under review', 'the subject', 'subject under review', 'tbd', 'todo', 'n/a', 'none', 'unknown', '<target>', '{target}', '${target}', '...']
if (!target || KNOWN_PLACEHOLDERS.includes(target.toLowerCase())) {
  throw new Error(
    `panel-review: args.target is missing, empty, or a placeholder (got: ${JSON.stringify(target)}). ` +
      `Refusing to spawn analysts against an unresolved subject. Re-invoke with a fully-specified target, e.g. ` +
      `Workflow({name: 'panel-review', args: {target: '<what to review>', question: '<what to answer>'}}).`
  )
}

// Echo the resolved subject immediately so the invoker can abort on a mismatch.
log(`panel-review target: ${target}`)
log(`panel-review question: ${question}`)

// Diversity is the whole point: distinct METHODS, not N copies of one lens.
// Override per-run via args.lenses = [{key, brief}, ...].
const lenses = (args && args.lenses && args.lenses.length)
  ? args.lenses
  : [
      { key: 'surface', brief: 'SURFACE pass only — read the README / docs / marketing. Do NOT clone or build. Report what is CLAIMED, and mark every claim as not-independently-verified (you cannot tell a claim from a fact at this depth).' },
      { key: 'deep', brief: 'DEEP DIVE — clone and inspect the actual source. Quantify with file:line evidence. Explicitly separate marketing copy from reproduced fact. Recount anything the surface pass asserted.' },
      { key: 'internals', brief: 'INTERNALS / ARCHITECTURE — build it if you can, read the persistence/state model, and hunt for capabilities that are CLAIMED but unwired/stubbed. Report the truth model: what is durable, what is local-only.' },
      { key: 'skeptic', brief: 'ADVERSARIAL SKEPTIC — assume the other analysts are wrong. Hunt for overclaims, stale numbers, fabricated specifics, and consensus that nobody verified. Query live sources (APIs, the actual repo) rather than trusting any prior text.' },
    ]

// ---------------------------------------------------------------------------
// Schemas — structured output is forced, so we synthesize from data, not prose.
// ---------------------------------------------------------------------------
const FINDINGS_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  properties: {
    lens: { type: 'string' },
    findings: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        properties: {
          claim: { type: 'string', description: 'a single falsifiable assertion' },
          evidence: { type: 'string', description: 'pointer: file:line, command + output, URL, or "none — asserted"' },
          confidence: { type: 'string', enum: ['high', 'med', 'low'] },
          loadBearing: { type: 'boolean', description: 'would the verdict change if this claim were false?' },
          selfVerified: { type: 'boolean', description: 'did YOU independently check this, or is it inherited/asserted?' },
        },
        required: ['claim', 'evidence', 'confidence', 'loadBearing', 'selfVerified'],
      },
    },
    notVerified: { type: 'string', description: 'what this analyst did NOT check (be honest)' },
  },
  required: ['lens', 'findings', 'notVerified'],
}

const VERDICT_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  properties: {
    verdict: { type: 'string', enum: ['confirmed', 'refuted', 'refined'] },
    reproduced: { type: 'boolean', description: 'did you INDEPENDENTLY re-run the check (read source / ran command / queried live API)?' },
    whatYouRan: { type: 'string', description: 'the exact command/inspection you performed — not reasoning from the claim text' },
    evidence: { type: 'string' },
    correctedValue: { type: 'string', description: 'if refuted/refined, the right answer' },
    selfGradeVerified: { type: 'boolean', description: 'true only if you genuinely reproduced; false if you ran out of means and inferred' },
  },
  required: ['verdict', 'reproduced', 'whatYouRan', 'selfGradeVerified'],
}

// ===========================================================================
// Phase 1 — ANALYZE: N independent analysts, blind to each other.
// ===========================================================================
phase('Analyze')
const analyses = (await parallel(
  lenses.map((l) => () =>
    agent(
      `You are the "${l.key}" analyst on a panel reviewing a subject. You are BLIND to the other analysts — do not assume what they found.\n\n` +
        `${l.brief}\n\n` +
        `TARGET:\n${target}\n\nQUESTION:\n${question}\n\n` +
        `Return findings as individual FALSIFIABLE claims, each with an evidence pointer, a confidence, whether it is load-bearing, and — critically — whether YOU personally verified it (selfVerified) vs inherited/asserted it. A confident synthesis is only as sound as its weakest unverified input, so do not inflate selfVerified. In notVerified, list what you could not check.`,
      { label: `analyze:${l.key}`, phase: 'Analyze', schema: FINDINGS_SCHEMA }
    )
  )
)).filter(Boolean)

// ---- Consolidate into a claims ledger (plain code — no agent, no barrier games)
const ledger = []
for (const a of analyses) {
  for (const f of a.findings || []) {
    ledger.push({ lens: a.lens, claim: f.claim, evidence: f.evidence, confidence: f.confidence, loadBearing: f.loadBearing, selfVerified: f.selfVerified })
  }
}
// Verify what matters: load-bearing first, then high-confidence, then asserted-but-unverified.
const priority = (c) => (c.loadBearing ? 0 : 2) + (c.selfVerified ? 0 : 1) - (c.confidence === 'high' ? 1 : 0)
const toVerify = ledger.slice().sort((a, b) => priority(a) - priority(b)).slice(0, maxVerify)
log(`${ledger.length} raw claims from ${analyses.length} analysts → verifying top ${toVerify.length}`)

// ===========================================================================
// Phase 2 — VERIFY: one adversarial reproducer per load-bearing claim.
// The reviewer's JOB is to refute, and it MUST re-run the check.
// ===========================================================================
phase('Verify')
const verdicts = (await parallel(
  toVerify.map((c, i) => () =>
    agent(
      // trace:BUG-509 | ai:claude — skeptics are anchored to the same resolved target as the analysts
      `Adversarially REPRODUCE this claim. Your job is to REFUTE it, not to agree.\n\n` +
        `You MUST independently re-run the check — read the actual source, run the command, query the LIVE API. Do NOT reason from the claim text or trust the cited evidence; reproduce it from the primary source. If you cannot reproduce it, the verdict is "refuted" and selfGradeVerified is false.\n\n` +
        `TARGET (the subject this claim is about — do not substitute another):\n${target}\n\nQUESTION:\n${question}\n\n` +
        `CLAIM: ${c.claim}\nCITED EVIDENCE (do not trust — reproduce): ${c.evidence}\n\n` +
        `Report what you ACTUALLY ran, the verdict (confirmed/refuted/refined), the corrected value if any, and an honest self-grade of whether you genuinely reproduced it.`,
      { label: `verify:${i}`, phase: 'Verify', schema: VERDICT_SCHEMA }
    ).then((v) => ({ ...c, ...v }))
  )
)).filter(Boolean)

// Highest-value signal: claims the analysts AGREED on but reproduction refuted.
const consensusButWrong = verdicts.filter((v) => v.verdict === 'refuted' && v.confidence !== 'low')

// ===========================================================================
// Phase 3 — RECONCILE: report with per-claim provenance + completeness critic.
// ===========================================================================
phase('Reconcile')
const report = await agent(
  `Write a brutally-honest RECONCILED report.\n\nTARGET:\n${target}\n\nQUESTION:\n${question}\n\n` +
    `You are given the analysts' findings and the adversarial verdicts. Rules:\n` +
    `1. For EVERY load-bearing claim, label provenance explicitly: confirmed-by-reproduction / refuted / refined / DELEGATED-UNVERIFIED. Never label a delegated-but-unreproduced finding as "verified" or "ground truth".\n` +
    `2. LEAD with any claim the analysts agreed on but reproduction refuted — consensus-but-wrong is the single most valuable finding (it is the failure mode "spawn N and average" produces).\n` +
    `3. State the verdict, then the recommended action, then the confidence.\n` +
    `4. END with a "What did we ALL miss?" completeness pass — a modality not run, a claim nobody reproduced, a source nobody read.\n\n` +
    `CONSENSUS-BUT-WRONG (lead with these): ${JSON.stringify(consensusButWrong)}\n\n` +
    `ANALYSES: ${JSON.stringify(analyses)}\n\nVERDICTS: ${JSON.stringify(verdicts)}`,
  { label: 'reconcile', phase: 'Reconcile' }
)

// ---- Optional: route still-disputed claims to a DIFFERENT vendor for true independence.
let crossVendorNote = ''
const disputed = verdicts.filter((v) => v.verdict !== 'confirmed' || v.selfGradeVerified === false)
if (crossVendor && disputed.length) {
  crossVendorNote =
    `\n\n---\n**Cross-vendor reproduction recommended** for ${disputed.length} claim(s) that were refuted/refined or not genuinely reproduced. ` +
    `For true independence (a different model, not a same-harness echo), route these via AIDA's brief surface, e.g. \`aida brief codex <SPEC> --note "reproduce: <claim>"\`. Disputed claims:\n` +
    disputed.map((d) => `- ${d.claim} → ${d.verdict}${d.correctedValue ? ` (corrected: ${d.correctedValue})` : ''}`).join('\n')
}

return report + crossVendorNote
