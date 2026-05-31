---
name: Never run a bulk --force operation when asked to scope it to a subset
description: If a destructive command has no per-file scope flag and the user asked for a subset, back up ALL affected files first or use an alternative path — filtering output does not change what the command did
type: feedback
propagation: scaffolding-pack
originSessionId: 123d0d20-197d-490d-a6fd-1332da826246
---
When the user asks me to apply a destructive operation to a *subset* of files, and the tool has no scope flag for that subset, I must NOT run the bulk version and rely on filtering to "verify scope." The operation runs against everything regardless of how I display its output.

**Why:** Burned on 2026-05-04 in `~/ai/paradox`. User asked me to `apply --force` only the two hook scripts. `aida scaffold apply --force` has no per-file selector. I:

1. Did the right thing first — extracted templates to a temp dir, manually copied just the two hooks. This worked.
2. Saw `scaffold status` still flag drift on the hooks (separate bug: extract vs apply disagree on whether the `# AIDA Generated:` header is embedded), and reflexively ran `aida scaffold apply --force | grep hooks` to "check." The grep filtered the *display* — the command had already overwritten all 4 drifted files: `CLAUDE.md` and `.claude/settings.json` (both untracked, not in any transcript, no backup) lost user customizations.
3. The `--dry-run` I ran *just before* clearly listed all 4 files. I didn't read it.

**How to apply:**
- If a destructive command has no scope flag and the requested scope is narrower than the command's natural scope: **stop**. State the gap. Pick one of: (a) back up *every* file the bulk operation will touch before running it, (b) refuse the bulk path and use a manual file-by-file alternative, (c) ask the user to confirm broader scope.
- Filtering output (`| grep`, `| head`, `--quiet`) never changes what a command writes. Don't conflate "what I see" with "what happened."
- Always read `--dry-run` output as a complete list, not "the file I'm focused on right now."
- For untracked files specifically (not in git): assume zero recovery options unless I personally backed them up. Transcripts only capture file content if Read/Write/Edit was called on them in a prior session.
- The carve-out from `feedback_dialog_routes_to_implementer` (settings.json/tooling tweaks OK inline in advisor mode) does NOT extend to bulk destructive operations on user-owned config files. Those still need explicit confirmation per file.
