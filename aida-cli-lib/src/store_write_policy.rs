// trace:TASK-3428 | ai:codex
use crate::Command;

pub(crate) fn command_triggers_per_write_auto_push(command: &Command) -> bool {
    // trace:STORY-1728 | ai:claude
    let Command::Registered(invocation) = command;
    invocation.policy().per_write_auto_push
}
