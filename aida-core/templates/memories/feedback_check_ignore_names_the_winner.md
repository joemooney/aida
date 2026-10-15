---
name: feedback_check_ignore_names_the_winner
description: "git check-ignore -v reports the winning rule under git's precedence order, not the only rule and not the causal one; attribute a path's ignored status by removing the candidate rule and re-testing, never by reading the tool's output alone."
type: feedback
propagation: scaffolding-pack
---
`git check-ignore -v <path>` prints **one** rule: the winner under git's precedence order. Among
patterns matching the same path, an in-tree `.gitignore` outranks `.git/info/exclude`, which outranks
`core.excludesFile`. But git first checks whether a *leading directory* is excluded, and a
directory-level match in any source wins for everything beneath it — so a `<dir>/` line in
`info/exclude` is named over a `.gitignore` rule that also covers the file. When two rules both cover
a path, the one reported reads like attribution and is not: it is a precedence fact about git, not
evidence about which rule causes the behavior or who authored it.

**Measured incident:** a status marker under a deny-by-default directory reported
`.git/info/exclude:19:<dir>/` when checked, which led to the conclusion "this is machine-local, a
clean clone could differ." Removing the candidate variable — a scratch repo containing only the
tracked `.gitignore` lines with an empty `info/exclude` — showed the tracked rule alone already
ignored the path on every clone. The behavior was designed and portable, not machine-local; the
tool's output had named the wrong layer as the cause.

**Why:** the correction runs in one direction only. "Machine-local" is the milder, easier-to-dismiss
finding; "designed into the repo for everyone" is the one that changes how seriously a defect is
sized. A tool that names *a* source is not a tool that proves *the* cause, and mis-attributing which
rule wins does not just get the mechanism wrong — it can make a real, repo-wide defect look like a
local quirk and get waved off.

**How to apply:** to claim rule X is why a path is ignored, construct the case *without* X and
re-check. This is cheap: `mkdir` a scratch repo, `git init`, copy in only the candidate `.gitignore`
lines, leave `info/exclude` empty (or vice versa to isolate the other layer), then run
`git check-ignore -v` again. If the path is still ignored, X was never the cause — something else was.
Apply the same discipline to any "which config/rule/precedence layer actually won" question, not just
`.gitignore`: a tool reporting the winner under some precedence order is answering "what fired first,"
not "what would happen if this one thing were absent."

Related: [[feedback_reconstruct_provenance_by_bracketing]]
