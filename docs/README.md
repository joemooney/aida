# AIDA documentation

Choose the guide for what you want to do.

## I'm using AIDA

Start with **[Why AIDA?](WHY-AIDA.md)**, then
**[install AIDA](../README.md#install)** and follow
**[Getting started](getting-started.md)**. AIDA supports Linux.

To try the commands in a new project after installation:

```sh
mkdir my-project
cd my-project
git init
aida init
aida add --type task --title "Make my first change" --status draft
aida list
```

Use the ID returned by `aida add` with `aida show <ID>`. Follow
[Your first project](first-project.md) to capture a small backlog and deliver
a change. These steps need no contributor documentation.

- [Everyday CLI commands](cli/01-getting-started.md) and [shaping specs](cli/02-specs.md)
- [Capture and delivery workflow](product-workflow.md), [lifecycle](lifecycle.md)
  and [requirement types](requirement-types.md)
- [Commit trailers](commit-trailer-convention.md), [Git workflow](git-workflow.md)
  and [history and reporting](cli/08-reporting.md)
- [Agent and MCP setup](agents/aida-mcp-install-matrix.md),
  [Codex](agents/codex-mcp-setup.md) and [Claude Code](using-aida-with-claude-code.md)
- [Configuration](configuration-guide.md), [operator setup](operator-setup.md)
  and [environment variables](environment-variables.md)
- [Troubleshooting and FAQ](troubleshooting.md)
- [CLI reference](cli/README.md) and [rendered guide library](showcase/library.html)

## I'm running AIDA at scale

Use the **[operator and power-user guides](advanced/README.md)** for multiple
nodes and hubs, shared deployments, forges, CI runners, unattended drains,
sessions, deputies and the mailbox.

## I'm working on AIDA

Start with **[Contributing](../CONTRIBUTING.md)** and the
**[contributor documentation](internal/README.md)** for architecture, repository
discipline, implementation plans, fixtures, research and presentations.

The [archive](archive/README.md) holds retired documentation and dated snapshots.
