# Feature Specification: Password Reset

**Feature Branch:** `003-password-reset`
**Created:** 2026-06-09
**Status:** In Progress (blocked)
**Input:** "A user who forgot their password can reset it via an emailed one-time link."

<!--
Representative Spec Kit `/speckit.specify` output (TASK-875 demo).
This feature depends on BOTH 001 (account lookup + re-hash) and
002 (revoke all existing sessions on reset). Both dependencies live
only in prose. There is no way, from inside this folder, to ask
"what is this feature blocked by, and is that blocker done?"
-->

## User Scenarios

### Primary Story
A user requests a reset for their email. The system emails a one-time, time-limited link. Following the link lets the user set a new password; all existing sessions are revoked.

### Acceptance Scenarios
1. **Given** a known email, **When** a reset is requested, **Then** a one-time token is emailed and expires in 1 hour.
2. **Given** a completed reset, **When** the user's old session token is presented, **Then** it is rejected (all sessions revoked).

## Requirements

### Functional Requirements
- **FR-001**: System MUST look up the account by email to issue a reset. **(Depends on `001-user-accounts`.)**
- **FR-002**: System MUST issue a one-time, 1-hour-expiry reset token.
- **FR-003**: System MUST re-hash the new password using the accounts hashing wrapper. **(Depends on `001-user-accounts`.)**
- **FR-004**: System MUST revoke ALL existing sessions for the account on a successful reset. **(Depends on `002-session-tokens` revoke path.)**

### Success Criteria
- **SC-001**: A reset link is single-use; a second follow returns 410 Gone.

## Key Entities
- **ResetToken**: `token`, `account_id`, `expires_at`, `consumed_at`.

## Dependencies
> Blocked by BOTH `001-user-accounts` (FR-001, FR-003) and `002-session-tokens` (FR-004). Implementation has NOT started pending those. Again: prose only — no graph to walk.
