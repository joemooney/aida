# Maintain Project Glossary

Build the ubiquitous language dictionary by scanning reqs and code for domain terms.

## Instructions

Follow the workflow in `.claude/skills/aida-glossary.md`:

1. Scan requirements (`aida list`, `aida search`) and key code paths for domain terms
2. Surface inconsistencies, synonyms, and ambiguous usage with file:line evidence
3. Propose canonical definitions for the team to align on
4. Save the glossary as a requirement (`--type meta`) or doc page, your choice with the user

Use when terminology drift is causing confusion or before a domain refactor.
