// trace:TASK-3428 | ai:codex
use crate::mailbox_root::MailboxResolveFailure;
impl std::fmt::Display for MailboxResolveFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { query } => write!(f, "message not found: {query}"),
            Self::AmbiguousPrefix { query, candidates } => write!(
                f,
                "message id prefix is ambiguous: {query}; lengthen the prefix to select one of: {candidates}"
            ),
        }
    }
}

impl std::error::Error for MailboxResolveFailure {}
