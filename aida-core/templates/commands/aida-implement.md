# Implement AIDA Requirement

Implement a requirement with full traceability.

## Usage

Invoke with: `/aida-implement <SPEC-ID>`

## Instructions

Follow the workflow in `.claude/skills/aida-implement.md`:

1. Load requirement: `aida show $ARGUMENTS`
2. Analyze scope and identify files to modify
3. Implement with traceability comments: `// trace:<SPEC-ID> | ai:claude:high`
4. Update requirement during implementation with `aida edit` and `aida comment add`
5. Create child requirements if needed with `aida add` and `aida rel add`
6. Mark complete: `aida edit <SPEC-ID> --status completed`
