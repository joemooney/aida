# Contained Codex pin and permission-profile compatibility

<!-- trace:BUG-2487 | ai:codex -->

The 2026-11-07 advisor ruling supersedes the original upgrade acceptance and
round-2 blocking decision below. Keep `codex-cli 0.166.0` and its existing
SHA-256 pin. Exec selects the generated profile with
`-c default_permissions="aida"`; `-P aida` remains exclusive to the separate
`codex sandbox` seat preflight. No package upgrade is required.

The launcher checks both argv scopes against recorded help before vendor
execution and names any unsupported flag. It never executes a lane-selected
binary on the host to discover options. The fixture's executable digest is
paired with `codex_auth::PROGRAM_SHA256` in the production-builder regression.

## Binary evidence, 2026-11-07

The installed npm vendored x86-64 musl executable reports `codex-cli 0.166.0`
and hashes to the existing production pin:
`b8a11fedf24b2dfb59a70c9e52b030f9a508dcd49cd2dd0d41956f4c785156b8`.

The requested release was downloaded into private scratch storage from npm's
`@openai/codex/0.167.0-linux-x64` package (the platform package alias used by
`@openai/codex@0.167.0`). Its tarball SHA-512 was checked against the registry
metadata before extraction:

```text
sha512-FNc7danfUng9Sp6w+Fh7kofNHHZpA6VMyNKWsWrS3OwHly69gywTWeiuwrUPXpJSho8Df4xqRsHfHRPzj7zYQg==
```

The extracted `vendor/x86_64-unknown-linux-musl/bin/codex` reports
`codex-cli 0.167.0`; its SHA-256 is:
`cdcb45aaa01d0f14e298985e09465aebccd47f64ecd298aea44814af7b62bbda`.
This is measured identity, not an approved replacement compatibility pin.
The global installation was not modified.

Every CLI probe used an empty temporary HOME/CODEX_HOME, an explicit minimal
environment and a five-second subprocess timeout. No operator credentials were
read. Results for the downloaded 0.167.0 executable:

| Arguments | Exit | Observation |
| --- | --- | --- |
| `--version` | 0 | `codex-cli 0.167.0` |
| `--help` | 0 | No `-P` / `--permission-profile` option |
| `exec --help` | 0 | No `-P` / `--permission-profile` option |
| `sandbox --help` | 0 | Has `-P, --permission-profile <NAME>` |
| `-P aida exec --help` | 2 | `error: unexpected argument '-P' found` |
| `exec -P aida --help` | 2 | `error: unexpected argument '-P' found` |

The installed 0.166.0 help has the same scope distinction. A sandbox-subcommand
capability must not be used as evidence that the top-level or exec parser
accepts a flag. The broken builder added `-P aida` before `exec`; the current
builder uses `-c default_permissions="aida"`.

Reproduce against a separately extracted binary without loading host config:

```python
import subprocess
import tempfile

binary = "/absolute/path/to/vendor/x86_64-unknown-linux-musl/bin/codex"
with tempfile.TemporaryDirectory() as home:
    env = {"HOME": home, "CODEX_HOME": home, "PATH": "/usr/bin:/bin"}
    for args in (["--version"], ["--help"], ["exec", "--help"],
                 ["sandbox", "--help"], ["-P", "aida", "exec", "--help"],
                 ["exec", "-P", "aida", "--help"]):
        result = subprocess.run([binary, *args], env=env, capture_output=True,
                                text=True, timeout=5)
        print(args, result.returncode, result.stdout, result.stderr)
```

## Fake-credential probe

The existing `aida-cli-lib/src/containment/tests/codex_auth_probe.py` was run
against the extracted binary with synthetic access/ID tokens, derived account
routing, an empty refresh string and a fresh delivery timestamp in a memfd.
Only a temporary in-memory copy of the probe was adapted: the expected version
was changed to 0.167.0 and the exec timeout increased from eight to 30 seconds.
The first eight-second attempt timed out; the 30-second retry passed:

```json
{"version":"codex-cli 0.167.0","executable_sha256":"cdcb45aaa01d0f14e298985e09465aebccd47f64ecd298aea44814af7b62bbda","bearer_matches":true,"account_matches":true,"fixture_http_status":400,"auth_unchanged":true,"secret_artifacts":false}
```

This confirms the reduced schema can route a synthetic request to the local
HTTP fixture and leave its credential file unchanged. The intentional HTTP
400 stops inference. The probe checks stdout/stderr and bounded private
runtime artifacts for the synthetic access token. It does not use production
`-P` argv, exercise the contained lane, prove absence of every refresh attempt,
or establish successful real-account inference. Host OAuth denial/deadline
enforcement still needs its own conformance evidence.

## Round 4 implementation and validation, 2026-11-07

The binding advisor ruling authorizes probing an exec-compatible config
selection and keeps the 0.166.0 pin. Empty-HOME probes found:

- `-c default_permissions="missing" exec ...` refuses with
  `default_permissions requires a [permissions] table`.
- A generated `[permissions.aida.filesystem]` table and
  `-c default_permissions="aida" exec ...` select the named profile.
- A local synthetic Responses server supplies exactly one `exec_command`
  call, then a final synthetic response. That call writes successfully in
  an allowed directory and fails with `EROFS` in a read-only directory.
  Both directories pass a write control under the same outer wrapper before
  Codex starts. No credentials or external inference are used.

Reproduce both the always-on pin/help regression and the opt-in installed
binary/profile probe:

```bash
cargo test -p aida-cli-lib --lib bug_2487 -- --nocapture
AIDA_REQUIRE_CODEX_INSTALLED_LIVE=1 cargo test -p aida-cli-lib --lib bug_2487 -- --nocapture
```

Lane validation: the opt-in command above passed both tests (2 passed,
0 failed), including the actual production-builder exec and sandbox write
probes. `cargo fmt --all -- --check` and `git diff --check` passed. The
execution-context guard reports 1241 exact legacy sites with zero additions.
The fixture uses `/tmp` explicitly rather than Cargo's RAM-backed test TMPDIR;
Codex startup is bounded at 40 seconds and helper probes at five seconds.

The installed test resolves the npm native leaf, checks its exact production
pin, compares fresh `exec --help` and `sandbox --help` against the fixture,
and passes the actual production builder argv to the exec parser and
synthetic write probe. The generated sandbox seat preflight separately passes
the same allowed/denied write check. Each subprocess has a timeout. It uses an
empty private HOME/CODEX_HOME and a temporary local HTTP listener. An additional
nested bwrap namespace replaces the parent native sandbox's mode-000 daemon
mask with an **empty** private 0700 directory, keeping host sockets hidden.
Fresh proc/dev mounts permit nested sandbox initialization. The outer fixture
allows both write-control paths; the selected Codex profile denies only one.
This is exec profile/argument evidence, not integrated AIDA C0-C4 conformance,
real-account authentication, or new credential-refresh evidence.

`codex_0166_help.json` records version, executable SHA-256 and unmodified
stdout from each help scope. Re-record from a newly measured binary whenever
changing the production pin; the always-on test fails if the digests differ.
It exercises the shared headless builder with model/effort and generated
transport flags, and verifies that sandbox-only and unknown flags fail closed
with the offending flag named.

Review disposition under the ruling: findings 1 and 4's upgrade demands are
superseded; findings 2 and 3 are implemented by the config selection, pre-launch
validation and paired regressions. Finding 6's decision is supplied by the
advisor. Finding 5's old re-pin subject remains historically inaccurate, but
the corrected implementation now supports the advisor's completing BUG-2487
attribution. The host should publish a new implementation commit to #3611
and use a squash/title subject describing the exec config fix and retained
0.166.0 pin, rather than claiming an upgrade. This native seat cannot rewrite
Git history or retitle/publish the PR. The existing merge hold is not cleared.

## Historical round 2 review disposition (superseded by advisor ruling)

The review of `cfbecb41250f` is accepted: AC1 (pin), AC2 (pre-launch flag
validation), AC3 (pin/argv regression), and AC4 (supported upgrade instructions)
remain unmet. This document is investigation evidence only. In particular,
the commit's claim to have re-pinned the binary is incorrect; neither that
subject nor its completing trailer establishes implementation or completion.

A fresh empty-HOME parser check in round 2 reproduced the same 0.167.0
SHA-256 above: both `-P aida exec --help` and `exec -P aida --help` exit 2,
while `sandbox --help` exits 0 and lists `--permission-profile`. No credentials
or inference were used in this repeat check.

Finding 6 is the blocking advisor decision, not permission to choose a new
containment interface. The disputed premise is that re-pinning alone to
0.167.0 fixes exec. An advisor must select an exec-compatible interface (or a
different measured binary) and revise acceptance before production rework.
Until then, findings 1–4 remain open and PR #3611 must stay held. Do not clear
the merge hold or mark BUG-2487 complete. Any later authorized publication of
this evidence must use a descriptive `docs(containment)` subject with
`(Refs BUG-2487)`, and correct the PR's completion attribution before merging.
