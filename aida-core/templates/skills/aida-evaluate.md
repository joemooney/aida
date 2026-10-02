---
name: aida-evaluate
description: Evaluate a requirement's quality using AI analysis.
allowed-tools:
  - Bash
  - Read
---

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

Agent: Loading FR-0042.

  Evaluating: FR-0042
  Title: Export reports to CSV
  Type: functional
  Status: draft

  Scoring clarity, completeness, testability, consistency and feasibility
  against the requirements and features already defined in the project.

Agent: ## Evaluation Results for FR-0042

  **Quality Score**: 6/10

  ### Strengths
  - The export format is named explicitly, so the output is unambiguous.
  - Scope is a single feature, appropriate for a functional requirement.

  ### Issues Found
  1. **[high]** missing_criteria: No acceptance criteria, so there is no
     pass/fail condition to verify against.
     Suggestion: State which columns the CSV contains and what happens
     when the report is empty.
  2. **[medium]** vague_language: "should handle large reports
     appropriately" has more than one interpretation.
     Suggestion: Replace it with an observable outcome.

  ### Suggested Improvements
  Export a finished report to CSV. The file contains one row per line item
  with the columns id, date, account, description and amount. An empty
  report still writes the header row. The export streams, so a report of
  any size is written without being held in memory.

  Rationale: turns two untestable phrases into observable outcomes, which
  raises testability and completeness without widening scope.

Agent: 6/10 is "Fair - some issues need addressing". Follow-up options:
  1. Improve description - apply the suggestion above
  2. Split into children - not recommended, scope is already one feature
  3. Find related - search for an existing export requirement
  4. Accept as-is - not recommended below a score of 7
  5. Add comment - record this evaluation without changing the description

User: 1, and record the evaluation too.

Agent: Applying the improvement, then recording the score.

  $ aida edit FR-0042 --description "Export a finished report to CSV. ..."
  FR-0042 updated

  $ aida comment add FR-0042 "AI Evaluation: Score 6/10 - no acceptance
    criteria and one vague phrase; description rewritten with per-column
    detail and an explicit empty-report case."
  comment added to FR-0042

  FR-0042 re-scores at 9/10 and is ready for implementation. Run
  /aida-plan FR-0042 when you want an implementation plan.
```
