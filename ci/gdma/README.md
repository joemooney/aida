# GitLab adapter for upstream CI

<!-- trace:STORY-2606 | ai:codex -->

`.github/workflows/ci.yml` owns the build/test/guard shell commands. The adapter
runs its `build` job under GitLab with the same lane, full-CI selection, job/step
environment, Bash error handling, and failure cleanup conditions. It interprets
only the expressions currently used by that job; unknown contexts, actions,
functions, shell substitutions or job/strategy/matrix/step settings refuse before
commands run. String comparisons and `startsWith` ignore case, as on GitHub.

The root `.gitlab-ci.yml` includes `.gitlab/ci/gdma.yml`. Its seven jobs run on
pushes to retained relay branches `ci/<positive attempt number>`. Existing
GitLab merge, scheduled, manual, Pages and release jobs retain their rules.
Literal job names (`gdma-ci: [lib-0]`, etc.) preserve the reference matrix's
GitLab display names while fitting the relay's closed inventory. The production
inventory test checks the complete include tree; the relay contract fixture
returns receipts containing these jobs. No matrix or deployment-variable rule
support is added to the receipt validator.

## Deployment configuration

Set these GitLab CI/CD variables before enabling relay pipelines:

| Variable | Purpose |
| --- | --- |
| `GDMA_CI_IMAGE` | Administrator-provisioned Linux builder image, preferably pinned by digest |
| `GDMA_RUNNER_TAG` | Runner tag for that builder and the required namespace capabilities |

The image must supply Bash, Python 3.11+ with PyYAML, Git, Rust/Cargo,
rustfmt/clippy, the cargo-nextest version selected in `ci.yml`, and the Linux
packages named in that workflow (including protoc, GTK/XCB development packages,
OpenSSL, bubblewrap, ripgrep and zsh). The runner must support the workflow's
nested user-namespace probes. Image creation, runner configuration, caches,
package credentials and namespace setup belong to the administrator.

The adapter maps checkout to GitLab's checkout, Rust/install/cache actions and
named apt/toolchain setup steps to this provisioned image, path filtering to a
local Git diff, and artifact upload to GitLab artifacts. It does not download
GitHub Actions or contact GitHub APIs. A newly introduced action must receive an
explicit mapping before it can run. Shell guards may still fetch from `origin`,
including the `aida-store` branch; provide the same repository history and store
access they need on upstream CI. Each job enforces a fresh checkout with
`GIT_STRATEGY: clone`, overriding the root pipeline's persistent-fetch policy.
The lane requests full history and fetches the default branch for diff/merge-base
checks. MR runs map GitLab's
target branch to `github.base_ref` and its event to `pull_request`; relay pushes
map to `push`. Missing/zero push baseline selects full CI conservatively.

Each job uses `$CI_PROJECT_DIR/target` for build output. `GITHUB_STEP_SUMMARY`
points to `target/ci-summary.md` for the existing shell reporting steps. Reports,
logs and timing files are copied to ignored `.ci-artifacts/` even after failure;
GitLab imports `.ci-artifacts/nextest/ci/junit-*.xml` as JUnit reports. A failed
command or timeout keeps the job red while status-qualified cleanup steps run.
Before execution, the adapter removes previous collected artifacts and matching
reports/logs/timings from `target`, so an early failure cannot upload old results.
The adapter applies `timeout-minutes` as a per-step subprocess cap (using the job
value as the default); GitLab's own `timeout: 40m` enforces the whole-job budget.

## Optional source policy

`GDMA_CHECK_SOURCES=1` runs `check-sources.py` before any upstream steps. Set all
of `GDMA_GITLAB_HOST`, `GDMA_CARGO_REGISTRY`, `GDMA_NPM_REGISTRY`, `CARGO_HOME` and
`NPM_CONFIG_REGISTRY`. The registry values are complete approved HTTPS URLs;
the GitLab value is a hostname. The checker accepts HTTPS/SSH Git sources only
on that host, requires Cargo's crates.io replacement to the approved registry,
and checks npm's resolved lockfile downloads against the approved registry path.
Cargo's canonical crates.io identity in `Cargo.lock` is allowed only with that
replacement. Registry URLs must not embed credentials. Errors omit source URLs.

The checker validates rather than rewrites configuration or lockfiles. A
restricted deployment must prepare its own source policy and lockfile overlay;
this upstream adapter supplies no registry addresses, credentials, package
configuration, runner/image setup, cache/keyring/session helpers, stack-size or
build-concurrency overrides. Source checking is opt-in so the unchanged public
lockfiles remain usable outside such a deployment.

## Local validation

```sh
python3 -m unittest tests/test_gdma_ci.py
python3 scripts/check-ci-gates.py --audit
cargo test --offline -p aida-cli-lib --lib ci_inventory::tests
```

The requested sccache changes from the reference are already in
`aida-cli-lib/src/dev_build.rs`: no automatic daemon management, and an explicit
child `RUSTC_WRAPPER` override wins when deciding whether to emit a restart hint.
`rustc_wrapper_for_prefers_the_child_override` covers that behavior.
