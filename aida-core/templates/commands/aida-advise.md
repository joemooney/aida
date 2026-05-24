# AIDA Advise

Headless advisor tier for the `--no-human=both` drain — judge a design-fork a
headless implementer punted on: resolve it from recorded principle/preference,
or escalate it to a human.

## Usage

```
/aida-advise   Invoked headlessly by the --auto-complete orchestrator; reads
               $AIDA_PUNT_REQUEST_FILE and writes $AIDA_PUNT_RESPONSE_FILE.
```

This command is not run by hand — the `--auto-complete --no-human=both`
orchestrator spawns it when a phase-1 implementer punts on a design-fork.

## Instructions

Follow the workflow in `.claude/skills/aida-advise.md`:

1. Read the punt request — `cat "$AIDA_PUNT_REQUEST_FILE"` (the fork, its
   options, the implementer's lean, and an ultraplan-grade context brief)
2. Classify the fork — Type A (recorded principle), B (recorded preference),
   or C (synthesized in-flight context)
3. Consult the recorded corpus — `docs/aida/discipline/`, `docs/plans/`, the
   spec graph, memories, existing codebase conventions
4. Decide — **resolve** only a Type A or recorded-B fork; **escalate**
   everything else. The default bias is escalate: if you would be guessing,
   escalate
5. Write the `PuntResponse` JSON to `$AIDA_PUNT_RESPONSE_FILE`
6. End — the headless run exits once the response file is written

The load-bearing rule: **when in doubt, escalate.** A confident-but-wrong
overnight decision is worse than the safe-default punt. A paused spec costs
the human five minutes; a wrong-but-merged call costs far more.

Pairs with `/aida-pickup` (the headless implementer that punted) and
`/aida-punt` (the punt mechanism this tier triages).
