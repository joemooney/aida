# Zero-token waiting for agent seats

Agent seats must wait in the shell or event substrate, not by repeatedly waking
the model. This applies especially to advisor/reviewer mailbox supervision and
long-running drains.

## Supported pattern

Use one shell-side source of actionable wakes:

- a harness `Monitor` over `aida watch --emit-wakes`;
- a background shell wait around `aida awaiting --notice`; or
- the TASK-1245 event-driven `aida advisor watch` heartbeat.

The waiting process consumes no model tokens while quiet. Invoke the seat only
after it emits actionable work. If the watcher fails, restart or repair that
shell process; do not add a model timer as a fallback.

Model-side `CronCreate`, `/loop`, and `ScheduleWakeup` are not supported for
mailbox polling. Do not stack recurring wake mechanisms.

## Cost model

Approximate tokens per wake as:

```text
tokens per wake ~= model calls per wake x context tokens per call
daily tokens ~= wakes per day x tokens per wake
```

Output length is usually the smaller term. A reply such as "mail tick: quiet"
still causes one or more inferences over the seat's loaded context, including
cache reads. The effective levers are therefore:

1. reduce wake count by waiting for actionable events;
2. reduce loaded context by ending/restarting long-lived sessions when safe;
3. only then optimize response length.

The 2026-09-21 incident behind BUG-1589 measured about 9.8B tokens across two
seats after recurring model-side ticks accelerated to thousands of wakes. About
99.8% of their usage was cache reads, despite most ticks producing only a quiet
response. The unresolved harness re-fire behavior was reported upstream at
<https://github.com/anthropics/claude-code/issues/96215>.

<!-- trace:BUG-1589 | ai:codex -->
