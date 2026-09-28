# History templates in tracked project config

Status: Accepted (Joe's storage decision, 2026-09-26; independent advisor signoff)

Trace: STORY-1477; sketch 01a0e08c-37d6-75e3-b4b7-2ef14d6ed106.

Named history layouts are presentation configuration. Store project templates in
tracked `.aida/config.toml`, under `[history.templates]`, with string names and
string values. Share changes through the normal code review path. A sibling
worktree uses its own project config, never the main worktree or orphan store.
Personal templates use the same schema in `~/.aida/config.toml`.

Reject aida-store META objects: presentation preferences should not expand the
requirement graph or acquire requirement lifecycle semantics. There is no third
writable `global:` scope. Lookup is user, then project, then immutable builtin;
an explicit qualifier bypasses precedence. Removal requires user: or project:.

This is additive: no migration, copying, or rewriting on read. Writes preserve
unrelated TOML comments and sections and replace the file atomically. Invalid
config is reported with file/key context. Save validates the inline layout,
target, fields and query, and renders the successful query before mutation.

Templates are a bounded whitelist, not an expression language. No shell,
environment, recursive expansion, or arbitrary object paths. Names follow
`[A-Za-z][A-Za-z0-9_-]*` (1–64 ASCII bytes); layouts allow at most 4096 bytes,
no literal controls/newlines, and rendered lines at most 16384 bytes. Substituted
controls and line separators become spaces. Chrono validates strftime syntax.
Historical title/priority are event-local and comment means a count/summary,
never a body. Decoder and cache schemas stay unchanged.

Custom layouts and ordered fields select full events, including for one spec.
Builtin full/oneline retain legacy CLI mode/rendering behavior. MCP explicitly
opts into human text with template; fields projects ordered JSON keys while
retaining the existing envelope. Default MCP JSON remains unchanged. No MCP
config mutations or saved filters/views are introduced.
