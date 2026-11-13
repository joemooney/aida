//! The ownership record and the facts the judge reads (STORY-2325 PR-A).
// trace:STORY-2325 | ai:claude

use crate::gate::canon::SpecKey;
use aida_core::process_probe::{Probe, ProcessIdentity};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A forge change (pull or merge request): the forge's name plus its number.
// trace:STORY-2325 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) struct ChangeId {
    pub forge: String,
    pub n: u64,
}

/// The tree a mailbox scope belongs to, named by the scope's anchor.
// trace:STORY-2325 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct MailAnchorKey(pub String);

/// The piece of work one record owns.
// trace:STORY-2325 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub(crate) enum WorkKey {
    Spec { spec: SpecKey },
    Change { change: ChangeId },
    Train { train: ChangeId },
    Review { change: ChangeId, head: String },
    MailScope { anchor: MailAnchorKey },
}

/// One piece of evidence that the holder is still running.
// trace:STORY-2325 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Witness {
    // trace:STORY-2537 trace:BUG-2458 | ai:codex
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub admission: Option<aida_core::proc_identity::HolderRecord>,
    pub kind: WitnessKind,
    pub recorded_at: DateTime<Utc>,
}

// trace:STORY-2325 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub(crate) enum WitnessKind {
    /// A process seen from the host: its host name, its pid plus start
    /// identity (`linux:<boot>:<ticks>`, empty when it could not be read),
    /// and the inode of the pid namespace the pid was read in.
    Process {
        host: String,
        id: ProcessIdentity,
        pid_ns: Option<u64>,
    },
    /// The holder's last heartbeat and the interval its ticker writes at.
    Heartbeat { at: DateTime<Utc>, every_secs: u64 },
}

// trace:STORY-2325 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Role {
    Implementer,
    Reviewer,
    Advisor,
    Train,
    Operator,
}

// trace:STORY-2325 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum TreeKind {
    Worktree,
    Clone,
}

/// The checkout a holder works in.
// trace:STORY-2325 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SeatTree {
    pub path: PathBuf,
    pub kind: TreeKind,
    pub branch: Option<String>,
}

/// Who holds the work, and the evidence that they still run.
// trace:STORY-2325 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Holder {
    pub seat: String,
    pub role: Role,
    /// The session lease id.
    pub session: String,
    pub tree: Option<SeatTree>,
    pub witnesses: Vec<Witness>,
}

/// Where the work is in its life.
// trace:STORY-2325 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "kebab-case")]
pub(crate) enum Phase {
    Leased,
    InPr {
        change: ChangeId,
        handed_off_at: DateTime<Utc>,
        /// Session that transferred this work; absent on older records.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        handed_off_by: Option<String>,
        head: String,
    },
    Reviewing {
        head: String,
    },
    Training {
        train: ChangeId,
    },
    Closed {
        why: String,
    },
}

/// The one record per work key. `generation` is the compare-and-swap counter
/// every write checks (PR-B).
// trace:STORY-2325 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Ownership {
    pub key: WorkKey,
    pub generation: u64,
    pub holder: Option<Holder>,
    pub phase: Phase,
}

/// The host the judge runs on, as the caller read it.
// trace:STORY-2325 | ai:claude
pub(crate) struct HostFacts<'a> {
    pub node_id: Option<String>,
    pub host: String,
    /// This host's boot id; `None` when it could not be read.
    pub boot: Option<String>,
    /// The inode of the pid namespace the reader runs in; `None` when it
    /// could not be read.
    pub pid_ns: Option<u64>,
    /// Probes one recorded process from the reader's namespace.
    pub probe: &'a dyn Fn(&ProcessIdentity) -> Probe,
}

/// What the reader found at the holder's tree. Mirrors
/// `pr_lease_takeover::SeatReading`, plus a path that is not a git checkout.
// trace:STORY-2325 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TreeReading {
    /// The holder records no tree, or its directory is gone.
    Missing,
    /// `dirty` lists every uncommitted change; `unpushed` names commits
    /// origin lacks; `head` is the tree's HEAD, `None` when unborn.
    Read {
        dirty: Vec<String>,
        unpushed: Vec<String>,
        head: Option<String>,
    },
    /// The directory exists but is not a git checkout (BUG-2198).
    NotAGitTree,
    /// The tree could not be read.
    Unreadable(String),
}

/// One witness's reading. `Unknown` abstains from the combination (A2).
// trace:STORY-2325 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WitnessReading {
    Alive,
    Dead(String),
    Unknown(String),
}

/// Whether the holder still runs, with the witness or proof behind it.
// trace:STORY-2325 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OwnerVerdict {
    Alive { witness: String },
    Dead { proof: String },
    Unknown { why: String },
}

/// What may be done with the work key.
// trace:STORY-2325 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reclaim {
    /// The key may be acquired. Never licenses touching the old tree (A7).
    Free,
    /// The holder is dead and its tree holds nothing the PR lacks (or is
    /// gone): the old session may be ended through the guarded path, with no
    /// `--force`.
    EndBySafeRule,
    /// The holder is dead but ending it could lose work: the tree is kept as
    /// salvage for a human.
    Refuse(String),
    /// The holder runs, or cannot be judged: it keeps the work.
    Keep(String),
}

// trace:STORY-2325 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Verdict {
    pub owner: OwnerVerdict,
    pub reclaim: Reclaim,
}
