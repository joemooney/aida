# Feature Specification: Session Tokens

**Feature Branch:** `002-session-tokens`
**Created:** 2026-06-05
**Status:** Implemented
**Input:** "Authenticated users get an opaque, revocable session token."

<!--
Representative Spec Kit `/speckit.specify` output (TASK-875 demo).
NOTE the per-feature ID scope: this feature's FR-001 is UNRELATED to
001-user-accounts' FR-001. Spec Kit has no project-wide ID namespace.
The cross-feature dependency on 001 (login calls find_account_by_email)
is expressed only in PROSE below — there is no machine-readable link.
-->

## User Scenarios

### Primary Story
A user submits credentials. The system verifies them against a stored account, mints an opaque token, and returns it. The token can later be presented to authenticate requests, and can be revoked.

### Acceptance Scenarios
1. **Given** a valid account, **When** correct credentials are submitted, **Then** a token is returned.
2. **Given** a revoked token, **When** it is presented, **Then** the request is rejected with 401.

## Requirements

### Functional Requirements
- **FR-001**: System MUST verify submitted credentials against a stored account. **(Depends on the User Accounts feature's `find_account_by_email` + password-hash verification — see `specs/001-user-accounts/`.)**
- **FR-002**: System MUST mint an opaque, high-entropy token on successful login.
- **FR-003**: System MUST support server-side revocation of a token.

### Success Criteria
- **SC-001**: Token verification adds under 5ms p95 overhead.

## Key Entities
- **Session**: `token` (opaque), `account_id` (FK to Account), `created_at`, `revoked_at`.

## Dependencies
> This feature CANNOT be implemented before `001-user-accounts` ships, because login reads accounts via that feature's lookup. This dependency is documented here in prose — Spec Kit has no relationship graph to make it queryable.
