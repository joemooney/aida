# Read-only lane CodeGraph queries

<!-- trace:STORY-2630 | ai:codex -->

The bundled CodeGraph 1.6.0 CLI opens databases writable, enables WAL, migrates
and heals them even for `callers`. A native Codex seat correctly refuses that
open (`✗ callers failed: unable to open database file`). This implements the
advisor's host-proxy option. Do not add a writable index path to any seat grant.

The host makes a consistent SQLite backup, including committed WAL pages, into
a private host state directory. Upstream queries run only against this disposable
snapshot. The primary database is opened with `mode=ro` for backup; the seat
opens neither database through the proxy. It can read the descriptor and access
the fixed Unix socket through its existing primary-index link. There is no new
filesystem write grant. Snapshot generations are never treated as immutable
while upstream can migrate or heal them. This is option (b), not a read-only
upstream CLI or an immutable client-side adapter.

Only `callers`, `callees`, `impact` and `status` are accepted. The server fixes
the executable, snapshot path and scrubbed environment, bounds requests/results
and kills query process groups after 15 seconds. Query options cannot change
project, executable, environment or run an indexer. Same-uid peers only. Each
server serializes queries and backups. Its periodic refresh detects primary
DB/WAL changes after host reindexing, even without a lane query. An active upstream reindex lock or changed input
during backup refuses that refresh and retains the preceding snapshot age.
New snapshots preserve the primary index's timestamp for freshness: capturing
an old index cannot make it appear fresh.

## Host installation

Run these steps **on the host**, using the selected candidate checkout. Install
a host-owned copy outside every seat write grant, so a lane cannot alter code
while the host executes it. This permits validation before merge and keeps the
primary source checkout clean. The service state directory must
be outside every seat's writable roots; in particular do not use `~/.aida`,
Cargo directories or a seat's `/tmp` for it.

```sh
install -d -m 700 /home/joe/ai/aida/.codegraph/proxy-state/bin
install -m 755 /path/to/candidate-checkout/scripts/codegraph/codegraph /home/joe/ai/aida/.codegraph/proxy-state/bin/codegraph
```

Create a user service `codegraph-query-aida.service`:

```ini
[Unit]
Description=Read-only lane queries against the host CodeGraph snapshot

[Service]
Type=simple
ExecStart=/usr/bin/python3 /home/joe/ai/aida/.codegraph/proxy-state/bin/codegraph serve --primary /home/joe/ai/aida --state /home/joe/ai/aida/.codegraph/proxy-state --upstream /home/joe/.codegraph/versions/v1.6.0/bin/codegraph
Restart=on-failure
RestartSec=5
UMask=0077
TimeoutStopSec=20

[Install]
WantedBy=default.target
```

```sh
systemctl --user daemon-reload
systemctl --user enable --now codegraph-query-aida.service
```

Keep the existing primary `codegraph-sync-aida.timer` and its original upstream
executable. Do not replace `~/.local/bin/codegraph`: unrelated projects and host
index maintenance keep using upstream. The host-owned `proxy-state/bin`
query wrapper must be selected explicitly inside the seat shell, in both native
and AIDA-contained postures. Launch finalization never derives executable PATH
entries from a seat-controlled `.codegraph` link. The host should provide the
reviewed absolute client path; existing and new seats use the same command:

```sh
PATH=/home/joe/ai/aida/.codegraph/proxy-state/bin:$PATH codegraph callers primary_index_link
PATH=/home/joe/ai/aida/.codegraph/proxy-state/bin:$PATH codegraph status
```

`--path` selects the **client's** project for descriptor discovery; it never
travels to the host as a query path. Clients derive the socket from the linked
`.codegraph` directory, never an arbitrary path from JSON. Output is upstream
JSON, including when `--json` is omitted. Limits default to 20 (max 100), and
impact depth defaults to 2 (max 10). No fallback to upstream writable opens.

The service refuses an existing socket so it cannot displace another server.
On crash, stop/check the service, confirm there is no live owner, remove only
`/home/joe/ai/aida/.codegraph/lane-query.sock`, then restart. A graceful stop
cleans its own socket. Old descriptor heartbeats cannot claim health after 30s.
Old snapshot directories left after a crash can be removed by the host while
the service is stopped. Do not run `serve` from a seat.

## Doctor and acceptance

`aida doctor --lane-host [--json]` reports primary-index age, snapshot capture
age, host observation age, and refresh errors from the host-owned descriptor.
The report includes a bounded status round trip (`query_reachable`), which
is not evidence of a successful graph query. Normal `doctor check codegraph`
reports missing tool, stale (>1h) snapshot, refresh failure and unreachable
query service or heartbeat. Before proxy deployment it retains the
existing direct-index diagnostic. The descriptor is read with a bounded
non-following regular-file handle.

After deploying, use a candidate AIDA binary containing the new doctor flag
and launch a **new contained Codex seat**. For an already-open seat, prefix PATH
as above. This permits acceptance evidence before the candidate is merged. Run:

```sh
codegraph callers primary_index_link --json
aida doctor --lane-host --json
```

Paste exact command/output into the PR body. Confirm callers includes
`link_worktree_runtime_state` and confirm with
`rg -n primary_index_link aida-cli-lib/src/worktree_gc.rs`. Record that the primary index and
`proxy-state` remain outside all seat writable roots; use the actual seat
sandbox evidence, not symlink routing, as the grant evidence. If socket IPC is
denied by the deployed sandbox, do not widen index writes or claim AC5.
The service must be deployed and that live query must pass before a completing
STORY-2630 trailer is appropriate.

## Validation

```sh
python3 -m unittest discover -s tests -p test_codegraph_proxy.py -v
```

The suite checks WAL-inclusive backups, source-age preservation, rejected
requests, socket protocol, time/output limits, primary preservation and actual
upstream caller/callee/impact results from a synthetic graph when bundled
CodeGraph is installed. Synthetic results are not AC5 evidence for the primary
index. Run Rust coverage through the lane runner:

```sh
aida lane job run -- cargo test --profile agent -p aida-cli-lib --lib -- codegraph::tests
```
