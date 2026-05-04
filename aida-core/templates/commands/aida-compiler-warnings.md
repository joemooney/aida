# Analyze Compiler Warnings

Categorize workspace warnings by risk and produce a prioritized cleanup plan.

## Instructions

Follow the workflow in `.claude/skills/aida-compiler-warnings.md`:

1. Run the appropriate build (`cargo build`, `tsc`, etc.) and capture warnings
2. Group warnings by category (unused, deprecation, type, lint) and risk level
3. Recommend a prioritized order (high-risk first, mechanical fixes last)
4. Offer to fix the easy ones inline or file requirements for larger cleanups

Use when the user wants to clean up warnings or assess overall code health.
