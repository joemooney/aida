# Daily Standup

Generate daily standup summary from recent activity.

## Instructions

1. Get yesterday's commits: `git log --oneline --since="yesterday"`
2. Extract requirement IDs from commit messages
3. Check current in-progress work: `aida list --status in-progress`
4. Format as standup: Done / In Progress / Blockers
