---
title: Substrate as bouncer, not passive rules
propagation: scaffolding-pack
---

# Substrate as bouncer, not passive rules

## The Principle

Fragile, passive developer guidelines (rules written in READMEs or memory files that agents and humans easily ignore or bypass in a hurry) fail under velocity. Instead, we build **enforcement mechanisms directly into the workspace substrate**. 

The substrate (Git hooks, IDE rules, local compiler checks) acts as an active **bouncer** that physicalizes boundaries, actively refusing invalid actions and forcing immediate correction.

## Key Habits

1. **Active Bouncers Over Passive Rules**: Do not just write "do not commit gitignored files" in a README. Wire a pre-commit hook that actively rejects the commit and educates the committer.
2. **Fail Fast and Hard**: Refuse compilation or commits immediately with highly clear, instructive errors pointing to the rationale.
3. **Escalation & Review Gates**: Combine local pre-commit gates with reviewer-phase pipeline gates (such as `TASK-480`) to ensure multi-layered enforcement.
4. **Deliberate Bypass**: Bouncers must provide a deliberate, explicit bypass option (such as `--allow-intermediate` or environment variables) so that they never become a blocker for emergency manual overrides, keeping control with the developer.
