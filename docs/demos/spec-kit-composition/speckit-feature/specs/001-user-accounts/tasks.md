# Tasks: User Accounts

**Spec:** [spec.md](./spec.md) | **Plan:** [plan.md](./plan.md)

<!-- Representative Spec Kit `/speckit.tasks` output. T-### ids scoped to THIS feature. -->

- [X] **T-001** Create `accounts` Postgres migration (id, email, password_hash, created_at). [FR-001, FR-002]
- [X] **T-002** Write failing contract test for register() happy path + duplicate-email 409. [SC-001]
- [X] **T-003** Implement `accounts::hash` argon2id wrapper. [FR-003]
- [X] **T-004** Implement `accounts::register()`. [FR-001, FR-002, FR-003]
- [X] **T-005** Implement `accounts::find_account_by_email()`. [FR-004]
- [X] **T-006** Wire `POST /accounts` HTTP handler. [FR-001]

_All tasks complete — feature `/implement`ed and merged 2026-06-04._
