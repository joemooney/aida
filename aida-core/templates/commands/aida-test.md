# Generate Tests

Generate tests linked to a requirement.

## Usage

Invoke with: `/aida-test <SPEC-ID>`

## Instructions

1. Load the requirement: `aida show $ARGUMENTS`
2. Find existing implementation via trace comments
3. Generate tests that verify the requirement's behaviors
4. Run the tests and report results
5. Link tests to the requirement with `Verifies` relationship
