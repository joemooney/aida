# /aida-doctor

Follow the workflow in `.claude/skills/aida-doctor.md`:

1. Run `aida doctor` for a read-only multi-agent drift report.
2. Use `aida doctor check <category>` for a focused diagnostic.
3. Use `aida doctor --heal` for prompted safe fixes.
4. Use `aida doctor --heal --yes` only when the operator wants batch cleanup.
5. Use `aida doctor heal orphan-branches --yes --force` only with explicit operator approval.
