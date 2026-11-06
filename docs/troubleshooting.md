# Troubleshooting and FAQ

## Where do I start?

[Install AIDA](../README.md#install), run `aida init` in your Git repository,
then create a spec with `aida add --type task --title "My first change"`.
Use `aida list` to find its ID and `aida show <ID>` to inspect it.
[Getting started](getting-started.md) explains the conversational workflow.

## Do I need a particular coding agent?

You can use the CLI directly. To connect an agent, follow the
[MCP setup matrix](agents/aida-mcp-install-matrix.md). For Codex, use the
[Codex setup guide](agents/codex-mcp-setup.md). MCP registration depends on
the client; check its configuration and restart it after changing the server.

## The command or option in a guide differs from my installation

Run `aida --version` and `aida <command> --help` to check the installed binary.
The [CLI reference](cli/README.md) explains the workflows; command help supplies
the exact options. Check that the binary on your PATH is the one you installed.

## Why does a read say the cache is stale?

The Git-canonical store holds the specs; the local cache is derived.
See [storage and cache commands](cli/10-storage.md) for inspection and explicit
refresh. Keep the canonical store intact when diagnosing a cache problem.

## Why can't my agent start or complete work?

Inspect `aida status`, `aida queue next` and `aida doctor`. A role, lease,
deferral or review gate can prevent pickup or completion. Follow the reported
reason and [work and autonomy reference](cli/03-work-autonomy.md). Shared and
unattended deployments have additional [operator guides](advanced/README.md).
