# Tasks: Session Tokens

**Spec:** [spec.md](./spec.md)

<!-- Representative Spec Kit `/speckit.tasks` output. T-### ids scoped to THIS feature. -->

- [X] **T-001** `sessions` migration (token, account_id FK, created_at, revoked_at). [FR-002]
- [X] **T-002** Failing contract test: login happy path + revoked-token 401. [SC-001]
- [X] **T-003** Implement `sessions::login()` — calls `accounts::find_account_by_email`. [FR-001]
- [X] **T-004** Implement opaque token minting (32 bytes CSPRNG). [FR-002]
- [X] **T-005** Implement `sessions::revoke()` + verify path. [FR-003]

_All tasks complete — feature `/implement`ed and merged 2026-06-08._
