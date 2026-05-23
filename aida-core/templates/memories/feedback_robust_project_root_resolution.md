---
title: Robust project root resolution fallback
propagation: scaffolding-pack
---

# Robust project root resolution fallback

## The Principle

When tools or actions (such as skill rendering) require finding the project root to locate static assets (like `.claude/skills/`), searching for a standard `.git` directory can fail in new workspaces, CI/CD runners, or custom deployment pipelines. 

To prevent runtime crashes and ensure seamless initializations, AIDA implements **Robust Project Root Resolution** with graceful fallback mechanics.

## Key Habits

1. **Graceful Fallbacks Over Hard Failures**: When parent directory traversal for `.git` fails, always fall back to the current working directory (`std::env::current_dir()`) as the assumed workspace root.
2. **Clear Error Context**: If the required asset (e.g., a specific skill file under `.claude/skills/`) is truly missing, report a precise error explaining where it was searched, rather than failing with generic "could not find project root" errors.
3. **Parity Between CLI and MCP Interfaces**: Maintain identical robust fallback logic across both the human-facing CLI and the machine-to-machine MCP surfaces to avoid behavior drift.
