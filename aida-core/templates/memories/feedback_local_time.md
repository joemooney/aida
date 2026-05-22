---
name: prefer local time in user-facing output
description: Joe prefers timestamps in his local timezone (not UTC) for any human-facing CLI / status / version output
type: feedback
propagation: scaffolding-pack
originSessionId: 1bf450af-f9e9-49d0-a098-79696b14cefc
---
Render timestamps in the user's local timezone for any human-facing output (`aida --version`, `aida status`, `aida list`, comment headers, "edited at", recent activity, etc.). Reserve UTC for machine-parseable surfaces (YAML on disk, oplog entries, JSON exports) where stable time is required.

**Why:** the user reads CLI output, not UTC offsets — a timestamp like `2026-05-09T19:18:29Z` forces mental conversion every time. Surfaced 2026-05-09 when the version banner showed UTC during a walkthrough.

**How to apply:** when displaying a `DateTime<Utc>` to the terminal, convert via `.with_timezone(&chrono::Local)` or use `Local::now()` directly. Keep the on-disk representation UTC; only the *display* is local. If unsure whether a path is human-facing or machine-facing, prefer local for `println!`/`eprintln!` and UTC for serializers.
