# Add AIDA Requirement

Add a new requirement to the database with AI evaluation.

## Instructions

Follow the workflow in `.claude/skills/aida-req.md`:

1. Ask user for requirement description (required) and optional: type, priority, feature, tags
2. Generate a concise title from the description
3. Add to database with `aida add --title "..." --description "..." --status draft`
4. Run AI evaluation (clarity, testability, completeness, consistency)
5. Offer follow-up actions: improve, split, link, or accept
