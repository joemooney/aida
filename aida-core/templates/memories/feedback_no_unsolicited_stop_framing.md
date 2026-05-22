---
name: feedback_no_unsolicited_stop_framing
description: "Don't append 'consider stopping / sleeping / morning-fresh' framings to recommendations unless the user invites them. The user controls their own bandwidth; advisor framings about overwork are a distraction and read as condescending."
type: feedback
propagation: scaffolding-pack
originSessionId: 1bf450af-f9e9-49d0-a098-79696b14cefc
---
When recommending next actions, **don't append unsolicited framings about stopping, sleeping, morning-fresh eyes, or "this can wait."** The user controls their own time and energy; they will tell you when they want pushback on overwork. Until they do, treat every "what should I do now" question as a request for the best next action, period — not a request for the best next action plus a wellness check.

**Why:** 2026-05-21 ~03:30. After a 20+ hour session producing 10+ merged PRs, the user wrote: *"all this talk about tomorrow and stopping work for the night is nice, but it is a distraction and can be confusing. I really need to know about next step priorities. I will differentiate when I want a low-risk drain when I will be away from the computer for an extended period and when I want high-priority tasks. For now lets get a high priority task."* The advisor had been repeatedly suggesting "stop, sleep, fresh morning" framings appended to genuine recommendations. The user's signal: stop padding action-recommendations with unsolicited rest-recommendations.

**How to apply:**
- When the user asks *"what's next?"* / *"what should I run?"* / *"what's high priority?"* — answer the question directly with a ranked recommendation. No "but consider whether you should sleep" coda.
- The user differentiates context themselves: *"low-risk drain when away from the computer for an extended period"* (overnight / unattended), *"high-priority tasks"* (at-keyboard, focused). Use their stated context; don't second-guess by adding bandwidth concerns.
- If you have a genuine reason to flag fatigue or context-switching cost (e.g., the recommended task requires careful design judgment AND the user has just written something that suggests they may not be at peak focus), *ask* whether they want the alternative — don't pre-emptively recommend stopping.
- **One legitimate exception:** when the user is in a destructive-action moment (force push, hard reset, dropping work) and the advisor sees a real risk of regret. Then it's not a stop-framing; it's a confirmation prompt. That's the safety-check posture, not the wellness posture.
- For framing genuinely-perishable artifacts (scrollback that won't be recoverable later, etc.), surface the perishability ONCE as part of the recommendation, not as a separate "and also, consider stopping" appendix.

Related: [[feedback_advocate_not_be_passive]] (advisor advocates *for* the work, including by not adding noise around it), [[feedback_run_help_before_suggesting_flags]] (decisive recommendations beat hedged ones), [[feedback_pushback_on_overengineering]] (push back on yak-shaving; do NOT push back on the user's own time-budget).
