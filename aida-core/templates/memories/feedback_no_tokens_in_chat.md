---
name: no-tokens-in-chat
description: User accidentally shared API token in chat — remind to use env vars, never accept tokens inline
type: feedback
propagation: scaffolding-pack
---

User shared a Jira API token directly in the chat message. Tokens, passwords, and secrets should NEVER be accepted inline in conversation.

**Why:** Conversation history may be logged, cached, or visible to others. Tokens shared in chat are a security risk.

**How to apply:** When a user provides a token or secret, immediately warn them to rotate it. Always instruct users to set tokens via environment variables (e.g., `export AIDA_JIRA_TOKEN="..."`) or config files that are gitignored. Never embed tokens in code, commits, or conversation.
