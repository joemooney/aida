# Tasks: Password Reset

**Spec:** [spec.md](./spec.md)

<!-- Representative Spec Kit `/speckit.tasks` output. NOT YET RUN through /implement. -->

- [ ] **T-001** `reset_tokens` migration (token, account_id FK, expires_at, consumed_at). [FR-002]
- [ ] **T-002** Failing contract test: request -> email -> follow -> set password -> sessions revoked. [SC-001]
- [ ] **T-003** Implement reset-token issuance. [FR-002]
- [ ] **T-004** Implement reset completion: re-hash via accounts wrapper. [FR-001, FR-003]
- [ ] **T-005** Revoke all sessions on reset (calls sessions::revoke). [FR-004]

_Status: not started. Blocked — see spec.md Dependencies._
