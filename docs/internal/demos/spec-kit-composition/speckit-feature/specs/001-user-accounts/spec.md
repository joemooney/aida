# Feature Specification: User Accounts

**Feature Branch:** `001-user-accounts`
**Created:** 2026-06-02
**Status:** Implemented
**Input:** "Users can register with an email + password and the system stores them securely."

<!--
Representative GitHub Spec Kit `/speckit.specify` output (hand-authored for
the AIDA composition-seam demo, TASK-875 — `specify` CLI not installed).
IDs follow Spec Kit's per-feature convention: FR-### functional requirements,
SC-### success criteria. NOTE: these IDs are scoped to THIS feature directory.
The `FR-001` here is a different requirement from the `FR-001` in 002/003.
-->

## User Scenarios

### Primary Story
A new user supplies an email and a password. The system validates the email is well-formed and unique, hashes the password, and persists the account. On success the user can subsequently authenticate.

### Acceptance Scenarios
1. **Given** no account exists for `a@b.com`, **When** the user registers with `a@b.com` + a valid password, **Then** an account is created and a 201 is returned.
2. **Given** an account already exists for `a@b.com`, **When** a second registration is attempted, **Then** a 409 conflict is returned and no second account is created.

## Requirements

### Functional Requirements
- **FR-001**: System MUST validate that the email is syntactically well-formed before persisting.
- **FR-002**: System MUST reject a registration whose email already exists (case-insensitive).
- **FR-003**: System MUST store passwords using a slow, salted hash (argon2id); plaintext MUST never be persisted.
- **FR-004**: System MUST expose a `find_account_by_email` lookup used by downstream authentication features.

### Success Criteria
- **SC-001**: A valid registration completes in under 300ms at p95.
- **SC-002**: No plaintext password appears in logs or storage (verified by audit).

## Key Entities
- **Account**: `id`, `email` (unique, lowercased), `password_hash`, `created_at`.
