# Implementation Plan: User Accounts

**Branch:** `001-user-accounts` | **Spec:** [spec.md](./spec.md)

<!-- Representative Spec Kit `/speckit.plan` output. -->

## Technical Context
- **Language:** Rust (stable), per constitution.
- **Storage:** Postgres `accounts` table.
- **Hashing:** `argon2` crate (argon2id).

## Constitution Check
- Library-first: `accounts` lives in `src/accounts/mod.rs`, HTTP shell in `src/http/accounts.rs`. PASS.
- Test-first: contract tests authored before handlers. PASS.

## Project Structure
```
src/
  accounts/
    mod.rs          # register(), find_account_by_email()
    hash.rs         # argon2id wrapper
  http/
    accounts.rs     # POST /accounts handler
tests/
  accounts_contract.rs
```

## Phase 0: Research
- argon2id parameters chosen: m=19456, t=2, p=1 (OWASP baseline). No open unknowns.

## Phase 1: Design
- `find_account_by_email(email) -> Option<Account>` is the seam downstream features (session-tokens, password-reset) depend on. Keep it stable.
