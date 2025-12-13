# Evaluate AIDA Requirement

Evaluate a requirement's quality using AI analysis.

## Usage

```
/aida-evaluate <SPEC-ID>
```

## Instructions

Follow the workflow in `.claude/skills/aida-evaluate.md`:

1. Load the requirement from the database using `aida show <SPEC-ID>`
2. Run AI evaluation for clarity, testability, completeness, and consistency
3. Display the quality score and any issues found
4. Offer follow-up actions: improve description, split into children, or accept as-is
