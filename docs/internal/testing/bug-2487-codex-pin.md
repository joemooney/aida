# Contained Codex pin and permission-profile compatibility

<!-- trace:BUG-2487 | ai:codex -->

BUG-2487 is blocked on a launch-interface decision. Re-pinning to the requested
`codex-cli 0.167.0` does **not** make the current `-P aida exec ...` argv valid.
Do not treat `npm i -g @openai/codex@0.167.0` as a working containment upgrade.
No production pin or launch argv was changed by this investigation.

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
accepts a flag. The current production builder adds `-P aida` before `exec`.

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

## Required decision and follow-up

Choose and authorize an exec-compatible way to select the generated permission
profile, or identify a different exact binary whose exec parser accepts the
current argv. The choice must preserve both model tool containment and the
separate sandbox seat preflight. A self-reported version or successful
`sandbox --help` alone is insufficient.

After that decision, pair the exact binary pin with the production argv builder
in an installed-binary regression, including distinct exec and sandbox scopes.
Pre-launch compatibility refusals must name the unsupported flag without
executing lane-selected vendor code on the host. Repeat the fake reduced-leaf
credential probe and host-run containment conformance before claiming support.
No C0-C4 pass credit, successful real-account authentication, or complete
no-in-lane-refresh proof is established by these probes.

## Round 2 review disposition, 2026-11-07

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
