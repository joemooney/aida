---
name: aida-req
description: Add a new requirement to the AIDA database with AI evaluation. Use when user wants to create a spec, add a feature request, or capture an idea.
allowed-tools:
  - Bash
  - Read
---

# AIDA Requirement Creation Skill

## Purpose

Add a new requirement to the AIDA requirements database with AI-powered evaluation feedback.

## When to Use

Use this skill when:
- User wants to add a new requirement or feature request
- User describes something they want the system to do
- User has an idea that should be captured as a requirement
- User asks to "add a requirement" or "create a spec"

## Current Project Context

- Features: !`aida feature list 2>/dev/null | head -20 || echo "none"`
- Recent requirements: !`aida list --format brief 2>/dev/null | tail -10 || echo "none"`

## Workflow

### Step 1: Gather Requirement Information

Ask the user for the following information (in conversational style):

1. **Description** (required): What should the system do? This can be:
   - A formal requirement: "The system shall..."
   - A question or idea to be formalized
   - A rough note that needs refinement

2. **Type** (optional, default: functional):
   - `functional` (FR) - System behaviors and features
   - `non-functional` (NFR) - Quality attributes (performance, security)
   - `user` (UR) - User needs/goals
   - `system` (SR) - Technical constraints

   Note: Use lowercase with hyphen for non-functional. "Feature" is NOT a type -
   use `--feature <name>` to assign a feature category.

3. **Priority** (optional, default: Medium):
   - High, Medium, Low

4. **Feature** (optional): Which feature area does this belong to?

5. **Tags** (optional): Comma-separated keywords

### Step 2: Add Requirement to Database

Use the `aida` CLI to add the requirement immediately:

```bash
aida add \
  --title "<generated-title>" \
  --description "<user-description>" \
  --type <type> \
  --priority <priority> \
  --status draft \
  --feature "<feature>" \
  --tags "<tags>"
```

**Title Generation**: Generate a concise title (5-10 words) from the description that captures the essence of the requirement.

### Step 3: Show Confirmation

After adding, display:
```
Requirement added: <SPEC-ID>
Title: <title>
Status: Draft (evaluation pending...)
```

### Step 4: Run AI Evaluation

Evaluate the requirement quality using the AI evaluation prompt. The evaluation should assess:

1. **Clarity** (1-10): Is the requirement clear and unambiguous?
2. **Testability** (1-10): Can this requirement be verified?
3. **Completeness** (1-10): Does it include all necessary information?
4. **Consistency** (1-10): Does it conflict with other requirements?

Provide:
- Overall quality score
- Issues found (if any)
- Suggestions for improvement
- Whether this should be split into multiple requirements

### Step 5: Offer Follow-up Actions

Based on the evaluation, offer:
- **Improve**: Let AI suggest improved description text
- **Split**: Generate child requirements if too broad
- **Link**: Suggest relationships to existing requirements
- **Accept**: Keep as-is and approve

## CLI Reference

```bash
# Add requirement (NOTE: use --tags not --tag)
aida add --title "..." --description "..." --type functional --priority high --status draft --tags "comma,separated"

# Show requirement details
aida show <SPEC-ID>

# Edit requirement
aida edit <SPEC-ID> --description "..."

# List features
aida feature list
```

## Integration Notes

- Requirements are stored in `requirements.yaml` or the configured project database
- SPEC-IDs are auto-generated based on type prefix configuration
- The GUI (aida-desktop) can be used to view and manage requirements with full AI features
