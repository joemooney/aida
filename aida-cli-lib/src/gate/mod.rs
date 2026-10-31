//! Gate authority: the decisions that say whether a ship, merge, hold,
//! verdict or seat action may proceed (STORY-1889). Everything under
//! `aida-cli-lib/src/gate/` is on the mandate's protected floor
//! ([`crate::gate::mandate_gates::GATE_PROTECTED_PATHS`]), so a change here needs a
//! human merge. Orchestration, display and parsing stay outside and call in.
// mandate-gate
// trace:STORY-1889 | ai:claude

pub(crate) mod approval;
pub(crate) mod branch_policy;
pub(crate) mod ci_freshness;
pub(crate) mod dispatch_auth;
pub(crate) mod dispatch_authority;
pub(crate) mod dispatch_deps;
pub(crate) mod dispatch_io;
pub(crate) mod dispatch_ledger;
pub(crate) mod dispatch_lock;
pub(crate) mod dispatch_reviewer_hold;
pub(crate) mod dispatch_types;
pub(crate) mod fleet_lanes;
pub(crate) mod hold_collect;
pub(crate) mod kernel_deps;
pub(crate) mod ledger;
pub(crate) mod mandate;
pub(crate) mod mandate_drive;
pub(crate) mod mandate_gates;
pub(crate) mod mandate_ship;
pub(crate) mod mandate_traces;
pub(crate) mod merge_hold_core;
pub(crate) mod operator_hold;
pub(crate) mod orchestrator_leaves;
pub(crate) mod pr_ship_batch;
pub(crate) mod relay_launch;
pub(crate) mod review_author;
pub(crate) mod review_authority;
// trace:TASK-1960 trace:STORY-b6b1278890774ef993d2ebc327862470-080 | ai:claude — attested-approval carry-over across clean base merges.
pub(crate) mod review_carryover;
pub(crate) mod review_marker;
pub(crate) mod review_verdict;
pub(crate) mod rework_hold_release;
pub(crate) mod role_authority;
pub(crate) mod seat_authority;
pub(crate) mod ship;
pub(crate) mod solo_review;
// BUG-HOME-048: completion trailers complete only the specs a PR covers.
pub(crate) mod trailer_coverage;
