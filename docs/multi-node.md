# Multi-node AIDA: identity, queue scope and published history

<!-- trace:TASK-B6B1278890774EF993D2EBC327862470-112 | ai:codex -->

When two machines show different work, first identify which queue and store each view is reading. Personal project queues, role-global queues, clone allocation and the derived cache have different scopes. Start with the [operator guide](operator-setup.md) for supported setup.

## Distinguish the queues

Per-project queues can be stored under `registry/queues/<user_id>.yaml` in the canonical store. User resolution accepts an explicit `--user`, then environment identity (`AIDA_USER`, `USER`, `USERNAME`) and a default fallback. Active roles and role routing also affect visible entries; cross-user role fallback can expose entries routed by another user.

Role-global queues live under `~/.aida/queue/<role>.yaml` on the machine. `aida queue list` can merge local project and active-role global views. `--local` selects only the project view; `--global` selects only the role-global view. A store push does not transfer the machine's global queue, grants or runtime evidence.

```bash
aida queue list --help
aida queue list --local
aida queue list --global
aida role status
```

## Check identity and routing

If one shell resolves to `joe` and another to `joseph`, their personal queue keys differ. Inspect the actual identity and filters before repairing anything:

```bash
echo "USER=$USER  AIDA_USER=$AIDA_USER"
aida queue list --local --all
```

Use a deliberate `AIDA_USER` where the same person needs consistent attribution; use distinct identities for different people. Do not treat matching strings as a multi-human security boundary. Role grants and authorized routing remain separate from display identity. Queue filtering and `--user` are documented by command help.

## Publish the history the other machine needs

AIDA mutations can commit store changes locally without proving that another machine has received them. Under ordinary transport, inspect and synchronize the store explicitly:

```bash
aida db status
aida db sync --push
# On the receiving machine:
aida db sync --pull
```

`aida pull` normally synchronizes code and store through its lifecycle; an enabled clone-local transport profile changes that contract to store-only checked fast-forward behavior. Inspect `aida config transport show` and the [transport reference](cli/07-project-setup.md#aida-config-transport) before choosing a path. Do not use raw Git as a workaround for a safety refusal.

A historical incident on 18 May 2026 involved a writer whose store was nine commits ahead of its remote. The other machine could only pull the published history. That diagnosis remains useful for project-queue history; it does not explain every empty role-global or filtered view.

## Automatic push policy

Ordinary store synchronization supports these configured modes:

```toml
[store.sync]
auto_push = "manual"
# auto_push = "session-end"
# auto_push = "per-write"
```

The default is manual. Session-end and per-write trigger best-effort publication at those points; inspect failures and remote acceptance rather than assuming publication always succeeded. The parsed `periodic` setting currently warns and falls back to manual. Automatic policy does not replace the clone transport contract.

## Cache and recovery

The read cache is derived and can report stale/deferred/busy context. `aida cache status` and explicit maintenance address projection freshness; a rebuild cannot recover missing unpublished history or recreate a different machine's role-global queue.

The clone's UUID binding and per-type allocation counters live in its common Git directory, shared by linked worktrees. WORK/HOME allocation policy is separate from queue user identity and from transport/privacy choices. Copying a store or old machine node file does not enroll an authoritative clone.

Preserve local store commits and operational evidence before changing attachments. A remote contains only accepted history. Use the [storage reference](cli/10-storage.md), [setup reference](cli/07-project-setup.md) and [multi-hub guide](multi-hub-sync.md) for the relevant layer rather than treating all disagreement as a cache issue.

## Check freshness and unpublished store history

Use `aida queue list --sync` for a one-shot pull before viewing the queue. When offline it warns and falls back to the local view; it cannot transfer a different machine's role-global queue.

In the default orphan-branch layout, refresh the store's remote-tracking ref and inspect locally unpublished commits:

```bash
git -C .aida-store fetch origin aida-store --quiet
git -C .aida-store log --oneline origin/aida-store..aida-store
```

Non-empty output means this clone has store commits absent from that fetched remote tip. Preserve those commits and use `aida db sync --push` through the project's configured transport policy. For sibling stores, use the configured store path.
