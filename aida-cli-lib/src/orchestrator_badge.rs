// trace:TASK-3428 | ai:codex
use crate::{auto_complete, OrchestratorBadge};

impl OrchestratorBadge {
    pub(crate) fn build(phase: Option<u8>, no_human_mode: Option<&str>) -> Self {
        let phase = match phase {
            Some(n) => match auto_complete::Phase::from_index(n as i32) {
                Some(p) => format!("auto:{n}/6 {}", p.slug()),
                None => format!("auto:{n}/6"),
            },
            None => "auto:?/6".to_string(),
        };
        OrchestratorBadge {
            phase,
            no_human: no_human_mode.map(|m| format!("no-human:{m}")),
            pause: "pause-here",
        }
    }
}
