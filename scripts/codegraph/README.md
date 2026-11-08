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
server serializes queries and backups, while bounded request workers answer
status independently and the accept loop publishes heartbeats. Its periodic refresh detects primary
DB/WAL changes after host reindexing, even without a lane query. An active upstream reindex lock or changed input
during backup refuses that refresh and retains the preceding snapshot age.
Queries continue against the preceding snapshot when refresh fails; status and
the descriptor expose the refresh error. A first snapshot remains mandatory.
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
query wrapper must be selected explicitly inside a native sandbox seat shell.
The AIDA-contained posture is not supported by this slice: its tmpfs root has
no mount for the host client, descriptor or socket. A host operator must supply
a separately reviewed read-only admission and contained-seat acceptance before
claiming that posture works. Launch finalization never derives executable PATH
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

The service refuses a live listener and non-socket paths. After a crash it
recovers only an unchanged socket whose connect probe returns ECONNREFUSED,
so a killed service can restart without manual socket removal. Other probe
errors refuse startup for host investigation. A graceful stop cleans its own socket. Old descriptor heartbeats cannot claim health after 30s.
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
non-following regular-file handle. When a proxy descriptor is present, doctor
also compares the PATH client with the bundled wrapper using a bounded read;
an upstream or different client on a lane produces a `client-routing` finding.
The primary host keeps upstream on PATH for index maintenance and skips this
lane-only client check. Reinstall
the matching reviewed host wrapper with a candidate AIDA build. This diagnostic
identifies client content only and grants no containment or launch authority.

For native-sandbox diagnostics after deployment, use a candidate AIDA binary
containing the new doctor flag. Existing and new seats both need the explicit
host-owned client path below (replace the host home with the actual deployment
path). Bare `codegraph` is not routed by launch setup. These diagnostics do not
satisfy the required AIDA-contained acceptance: the host operator must first
implement and validate its read-only client/descriptor/socket admission. Run:

```sh
/home/joe/ai/aida/.codegraph/proxy-state/bin/codegraph callers primary_index_link --json
aida doctor --lane-host --json
```

Paste exact command/output into the PR body. Confirm callers includes
`link_worktree_runtime_state` and confirm with
`rg -n primary_index_link aida-cli-lib/src/worktree_gc.rs`. Record that the primary index and
`proxy-state` remain outside all seat writable roots; use the actual seat
sandbox evidence, not symlink routing, as the grant evidence. If socket IPC is
denied by the deployed sandbox, do not widen index writes or claim AC5.
Host operator Joe owns deployment, delivery of the reviewed absolute client
path to lane prompts, and the remaining contained-seat AC2/AC5 run. The current
shared navigation prompt describes query selection but does not install or
select this client. Do not treat a native run as AIDA-contained evidence.

Status requests bypass the query/backup lock. Up to sixteen bounded request
workers isolate stalled peers, with a two-second request deadline. Status bypasses
the work lock but shares worker capacity: with all sixteen workers occupied,
connections (including doctor status probes) are closed; callers can retry. A
doctor reachability failure can therefore indicate temporary saturation. Query lock acquisition is bounded
at fifteen seconds. Heartbeats publish every three seconds during backups and queries. Client
deadlines cover lock acquisition, backup and upstream execution (50 seconds).

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
