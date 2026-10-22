//! Gate authority: the decisions that say whether a ship, merge, hold,
//! verdict or seat action may proceed (STORY-1889). Everything under
//! `aida-cli-lib/src/gate/` is on the mandate's protected floor
//! ([`crate::mandate_gates::GATE_PROTECTED_PATHS`]), so a change here needs a
//! human merge. Orchestration, display and parsing stay outside and call in.
// mandate-gate
// trace:STORY-1889 | ai:claude

pub(crate) mod branch_policy;
pub(crate) mod merge_hold_core;
pub(crate) mod operator_hold;
pub(crate) mod role_authority;
pub(crate) mod seat_authority;
