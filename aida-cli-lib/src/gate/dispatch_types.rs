//! Plain data the dispatch facade hands the gate: the forge CI state the flow
//! ship selector matches on and the drive ledger's row and outcome
//! (TASK-b6b1278890774ef993d2ebc327862470-013). They live on the floor so no
//! edit outside `gate/` changes the code the gate runs on them. Callers, `forge`
//! and `mandate_drive` included, import them from the floor
//! (`crate::gate::dispatch_types`); nothing outside `gate/` re-exports them.
// mandate-gate
// trace:TASK-b6b1278890774ef993d2ebc327862470-013 | trace:TASK-b6b1278890774ef993d2ebc327862470-028 | ai:claude

use serde::{Deserialize, Serialize};

// trace:TASK-b6b1278890774ef993d2ebc327862470-013 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiState {
    /// No CI configured / no forge — drain treats like `lifecycle:no-ci-wait`.
    None,
    Pending,
    Running,
    Success,
    Failed,
}

/// How one action ended.
// trace:TASK-b6b1278890774ef993d2ebc327862470-013 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Outcome {
    Done,
    Routed,
    Gated,
    Refused,
    Skipped,
}

impl Outcome {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Routed => "routed",
            Self::Gated => "gated",
            Self::Refused => "refused",
            Self::Skipped => "skipped",
        }
    }
}

/// One `MandateDriveAction` row.
// trace:STORY-1700 | trace:TASK-b6b1278890774ef993d2ebc327862470-013 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ActionRow {
    pub lane: String,
    pub subject: String,
    pub action: String,
    pub outcome: Outcome,
    pub why: Option<String>,
}

impl ActionRow {
    pub(crate) fn new(
        lane: &str,
        subject: &str,
        action: &str,
        outcome: Outcome,
        why: Option<String>,
    ) -> Self {
        Self {
            lane: lane.into(),
            subject: subject.into(),
            action: action.into(),
            outcome,
            why,
        }
    }
}
