# Rework-brief craft

Read the actual reviewer verdict, verify each blocking finding against the diff
and acceptance, then name the affected file/symbol, failing or missing test,
observed behavior, concrete required change, and verdict path. Requeue the same
spec for its implementation role and verify the brief is visible with `aida
brief list --for-agent <agent>`. A fresh implementer must be able to tell what
failed, where, how to reproduce it, what outcome is required, and what evidence
will prove the fix. <!-- trace:STORY-1351 | ai:codex -->
