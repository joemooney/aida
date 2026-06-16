---
name: aida-evaluate
description: Evaluate a requirement's quality using AI analysis. Use when user wants to assess clarity, testability, completeness, or consistency of a requirement.
allowed-tools:
  - Bash
  - Read
---
<!-- AIDA Generated: v2.0.0 | checksum:dbace384 | DO NOT EDIT DIRECTLY -->
<!-- To customize: copy this file and modify the copy -->


# AIDA Requirement Evaluation Skill

## Purpose

Evaluate a requirement's quality using AI analysis, providing feedback on clarity, testability, completeness, and consistency.

## When to Use

Use this skill when:
- User wants to evaluate a specific requirement's quality
- User asks to "evaluate", "assess", or "review" a requirement
- User wants AI feedback on a requirement before implementation
- Triggered by `/aida-evaluate <SPEC-ID>` command

## Workflow

### Step 1: Load the Requirement

Get the requirement details from the database:

```bash
aida show <SPEC-ID>
```

Display the requirement summary:
```
Evaluating: <SPEC-ID>
Title: <title>
Type: <type>
Status: <status>
```

### Step 2: Build Evaluation Context

The AI evaluation considers:
- **Project context**: Total requirements, features, types defined
- **Requirement details**: Title, description, type, status, priority, relationships
- **Related requirements**: Parent/child relationships, same-feature requirements

### Step 3: Run AI Evaluation

Evaluate the requirement against these quality criteria:

1. **Clarity** (1-10): Is the requirement clearly stated and unambiguous?
   - Look for vague language ("should", "may", "some", "appropriate")
   - Check for undefined terms or jargon
   - Ensure single interpretation possible

2. **Completeness** (1-10): Does it have sufficient detail for implementation?
   - Acceptance criteria present or inferrable
   - Edge cases considered
   - Dependencies identified

3. **Testability** (1-10): Can this requirement be verified/tested?
   - Measurable success criteria
   - Observable outcomes
   - Clear pass/fail conditions

4. **Consistency** (1-10): Does it align with related requirements?
   - No conflicts with existing requirements
   - Terminology consistent with project
   - Appropriate scope for type

5. **Feasibility**: Is it realistic and achievable?

### Step 4: Generate Evaluation Response

The evaluation should produce a JSON structure:

```json
{
  "quality_score": <1-10>,
  "issues": [
    {
      "type": "<vague_language|missing_criteria|ambiguous|incomplete|inconsistent|untestable>",
      "severity": "<low|medium|high>",
      "text": "<description of the issue>",
      "suggestion": "<how to fix it>"
    }
  ],
  "strengths": ["<strength1>", "<strength2>"],
  "suggested_improvements": {
    "description": "<improved description text if needed, or null>",
    "rationale": "<why this improvement helps>"
  }
}
```

### Step 5: Display Results

Present the evaluation results clearly:

```
## Evaluation Results for <SPEC-ID>

**Quality Score**: X/10

### Strengths
- <strength1>
- <strength2>

### Issues Found
1. **[severity]** <issue type>: <description>
   Suggestion: <how to fix>

### Suggested Improvements
<improved description or "No improvements needed">
```

### Step 6: Offer Follow-up Actions

Based on the evaluation, offer these options:

- **Improve Description**: Apply the suggested improvements to the requirement
  ```bash
  aida edit <SPEC-ID> --description "<improved description>"
  ```

- **Split into Children**: If the requirement is too broad, generate child requirements
  (Use `/aida-implement` or the generate children AI feature)

- **Find Related**: Search for potential duplicates or related requirements
  ```bash
  aida search "<key terms from description>"
  ```

- **Accept**: Keep the requirement as-is if quality is acceptable (score >= 7)

- **Add Comment**: Add evaluation notes as a comment
  ```bash
  aida comment add <SPEC-ID> "AI Evaluation: Score X/10 - <summary>"
  ```

## Quality Score Guidelines

- **9-10**: Excellent - Ready for implementation
- **7-8**: Good - Minor improvements possible
- **5-6**: Fair - Some issues need addressing
- **3-4**: Poor - Significant rework needed
- **1-2**: Critical - Major revision required

## CLI Reference

```bash
# Show requirement details
aida show <SPEC-ID>

# Edit requirement
aida edit <SPEC-ID> --description "..."

# Add evaluation comment
aida comment add <SPEC-ID> "..."

# Search for related requirements
aida search "<terms>"

# List requirements by status
aida list --status draft
```

## Example Session

```
User: /aida-evaluate FR-0042