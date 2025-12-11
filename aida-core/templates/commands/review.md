# Review Requirement

Review a specific requirement for quality and completeness.

## Usage

Invoke with: `/review <SPEC-ID>`

## Instructions

1. Load the requirement: `aida show $ARGUMENTS`
2. Evaluate the requirement for:
   - Clarity: Is it unambiguous?
   - Testability: Can it be verified?
   - Completeness: Does it have all necessary information?
3. Suggest improvements if needed
4. Offer to update the requirement with suggested changes

## Output Format

```
## Review: [SPEC-ID] - [Title]

### Quality Assessment
- Clarity: X/10
- Testability: X/10
- Completeness: X/10

### Issues Found
- Issue 1
- Issue 2

### Suggested Improvements
[Improved description text]

### Actions
- [ ] Update description
- [ ] Add acceptance criteria
- [ ] Approve requirement
```
