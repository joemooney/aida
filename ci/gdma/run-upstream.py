#!/usr/bin/env python3
"""Run ci.yml shell checks under GitLab.

trace:STORY-2606 | ai:codex
"""

import argparse
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import tempfile

import yaml


ROOT = Path(__file__).resolve().parents[2]
TOKEN = re.compile(r"\s*(\|\||&&|==|!=|[!(),]|'[^']*'|[A-Za-z_][A-Za-z0-9_.]*)")
INSTALL_STEPS = {
    "Install Rust toolchain", "Pin stable toolchain & verify cargo",
    "Install Linux dependencies", "Install zsh (shell-wrapper test matrix)",
    "Install awk engines",
}
# Runner-owned shell inputs must be explicitly supplied, never inherited by accident.
# trace:STORY-2606 | ai:codex
RUNNER_VARIABLES = {"GITHUB_STEP_SUMMARY", "RUNNER_TEMP"}
RUNNER_REFERENCE = re.compile(r"\$\{?((?:RUNNER_|GITHUB_)[A-Za-z0-9_]+)")
ACTIONS = {
    "actions/checkout", "dorny/paths-filter", "dtolnay/rust-toolchain",
    "Swatinem/rust-cache", "taiki-e/install-action", "actions/upload-artifact",
}
ARTIFACT_PATTERNS = (
    "test-run.log", "test-timings.json", "changed-files.txt",
    "nextest/ci/junit-*.xml", "cargo-timings/*", "ci-summary.md",
)
SUBSTITUTIONS = {
    "matrix.shard": "${GDMA_LANE}",
    "github.base_ref": "${GDMA_BASE_REF}",
    "github.event_name": "${GDMA_EVENT_NAME}",
}


class Expression:
    def __init__(self, text, context):
        text = text.strip()
        if text.startswith("${{") and text.endswith("}}"):
            text = text[3:-2].strip()
        self.tokens = []
        position = 0
        while position < len(text):
            match = TOKEN.match(text, position)
            if not match:
                raise ValueError(f"Unsupported workflow expression near {text[position:]!r}")
            self.tokens.append(match.group(1))
            position = match.end()
        self.position = 0
        self.context = context

    def take(self, token):
        if self.position < len(self.tokens) and self.tokens[self.position] == token:
            self.position += 1
            return True
        return False

    def require(self, token):
        if not self.take(token):
            raise ValueError(f"Expected {token!r} in workflow expression")

    def value(self):
        result = self.disjunction()
        if self.position != len(self.tokens):
            raise ValueError("Unexpected workflow expression tokens")
        return result

    def disjunction(self):
        result = self.conjunction()
        while self.take("||"):
            right = self.conjunction()
            result = result or right
        return result

    def conjunction(self):
        result = self.comparison()
        while self.take("&&"):
            right = self.comparison()
            result = right if result else result
        return result

    def comparison(self):
        result = self.atom()
        for operator in ("==", "!="):
            if self.take(operator):
                right = self.atom()
                # GitHub compares expression strings without regard to case.
                if isinstance(result, str) and isinstance(right, str):
                    result, right = result.lower(), right.lower()
                equal = result == right
                return equal if operator == "==" else not equal
        return result

    def atom(self):
        if self.take("!"):
            return not self.atom()
        if self.take("("):
            result = self.disjunction()
            self.require(")")
            return result
        if self.position >= len(self.tokens):
            raise ValueError("Incomplete workflow expression")
        token = self.tokens[self.position]
        self.position += 1
        if token.startswith("'"):
            return token[1:-1]
        if self.take("("):
            if token in {"cancelled", "always", "success"}:
                self.require(")")
                return self.context[token]
            if token == "startsWith":
                left = self.disjunction()
                self.require(",")
                right = self.disjunction()
                self.require(")")
                return str(left).lower().startswith(str(right).lower())
            raise ValueError(f"Unsupported workflow function {token}")
        if token not in self.context:
            raise ValueError(f"Unknown workflow context {token}")
        return self.context[token]


def condition(text, context):
    return Expression(text, context).value()


def shell(text):
    def substitute(match):
        key = match.group(1).strip()
        if key not in SUBSTITUTIONS:
            raise ValueError(f"Unsupported shell interpolation {key}")
        return SUBSTITUTIONS[key]
    return re.sub(r"\$\{\{(.*?)\}\}", substitute, text)


def step_command(step):
    command = shell(step["run"])
    if step.get("name") == "Live bwrap OS-wrap confinement gate":
        # Host namespace policy is provisioned by the administrator, like the
        # equivalent preparation in Install Linux dependencies. Keep the probe.
        command = command.replace(
            "sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0 || true\n", "", 1)
    return command


def full_ci(base):
    try:
        result = subprocess.run(
            ["git", "diff", "--name-only", "-z", f"{base}...HEAD"],
            cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True,
        )
    except subprocess.CalledProcessError:
        # Missing objects or merge bases must never suppress the suite.
        print("Baseline unavailable; selecting full CI", file=sys.stderr)
        return True
    paths = result.stdout.decode().split("\0")
    return any(path and not (
        path.startswith("docs/") or path.endswith(".md") or
        path in {"LICENSE", ".gitignore"}
    ) for path in paths)


def event_name(environ):
    return "pull_request" if environ.get("CI_PIPELINE_SOURCE") == "merge_request_event" else "push"


def context(lane, full, failed=False, environ=None):
    environ = {} if environ is None else environ
    return {
        "matrix.shard": lane, "runner.os": "Linux",
        "runner.environment": "github-hosted",
        "github.event_name": event_name(environ),
        "github.event.pull_request.base.sha": environ.get("CI_MERGE_REQUEST_DIFF_BASE_SHA", ""),
        "github.event.merge_group.base_sha": "",
        "github.event.before": environ.get("CI_COMMIT_BEFORE_SHA", ""),
        "steps.changes.outputs.full_ci": "true" if full else "false",
        "cancelled": False, "always": True, "success": not failed,
        "true": True, "false": False,
    }


def step_environment(step, current, parent):
    environment = parent.copy()
    for key, value in step.get("env", {}).items():
        if isinstance(value, str) and value.startswith("${{"):
            value = condition(value, current)
        if isinstance(value, str) and "${{" in value:
            raise ValueError("Unsupported embedded environment expression")
        environment[key] = str(value)
    return environment


def validate_job(job, current):
    # trace:STORY-2606 | ai:codex
    if set(job) - {"name", "runs-on", "defaults", "timeout-minutes", "env", "strategy", "steps"}:
        raise ValueError("Unsupported upstream job settings")
    strategy = job.get("strategy", {})
    if set(strategy) - {"fail-fast", "matrix"}:
        raise ValueError("Unsupported upstream strategy settings")
    matrix = strategy.get("matrix", {})
    if set(matrix) - {"os", "shard"}:
        raise ValueError("Unsupported upstream matrix settings")
    if matrix.get("os", ["ubuntu-latest"]) != ["ubuntu-latest"]:
        raise ValueError("Unsupported upstream matrix OS")
    # trace:STORY-2672 | ai:codex
    # GitLab always uses its provisioned image; accept only the reviewed GitHub
    # routing policy, never arbitrary runner expressions.
    guarded_runner = '${{ vars.AIDA_DELL1_CI == \'true\' && (github.event_name == \'push\' || (github.event_name == \'pull_request\' && github.event.pull_request.head.repo.full_name == github.repository)) && contains(fromJSON(\'["lib-0","lib-1","cli-0","cli-1"]\'), matrix.shard) && fromJSON(\'["self-hosted","Linux","X64","aida-dell1"]\') || matrix.os }}'
    if job.get("runs-on", "${{ matrix.os }}") not in ("${{ matrix.os }}", guarded_runner):
        raise ValueError("Unsupported upstream runner")
    if set(job.get("defaults", {})) - {"run"}:
        raise ValueError("Unsupported upstream defaults")
    allowed = {"name", "id", "if", "run", "uses", "with", "env", "shell", "timeout-minutes"}
    step_environment(job, current, {})
    if job.get("defaults", {}).get("run", {}) not in ({}, {"shell": "bash --noprofile --norc -e -o pipefail {0}"}):
        raise ValueError("Unsupported upstream run defaults")
    for step in job["steps"]:
        if set(step) - allowed:
            raise ValueError("Unsupported upstream step settings")
        condition(step.get("if", "true"), current)
        step_environment(step, current, {})
        if "uses" in step:
            action = step["uses"].split("@", 1)[0]
            if action not in ACTIONS:
                raise ValueError("Unmapped upstream action")
            if action == "dorny/paths-filter":
                settings = step.get("with", {})
                filters = yaml.safe_load(settings.get("filters", ""))
                if (step.get("id") != "changes" or
                        settings.get("predicate-quantifier") != "some-with-excludes" or
                        filters != {"full_ci": ["**", "!docs/**", "!**/*.md", "!LICENSE", "!.gitignore"]}):
                    raise ValueError("Unmapped upstream path-filter policy")
        elif "run" in step:
            if step.get("shell", "bash") not in {"bash", "bash --noprofile --norc -e -o pipefail {0}"}:
                raise ValueError("Unsupported upstream shell")
            command = step_command(step)
            if step.get("name") not in INSTALL_STEPS:
                if re.search(r"\b(?:sudo|apt-get)\b", command):
                    raise ValueError("Unmapped upstream image installation step")
                unknown = set(RUNNER_REFERENCE.findall(command)) - RUNNER_VARIABLES
                if unknown:
                    raise ValueError(f"Unsupported runner variables: {', '.join(sorted(unknown))}")
        else:
            raise ValueError("Unmapped upstream step")


def clean_artifacts():
    """Remove previous-run reports before any command can fail or skip tests."""
    # trace:STORY-2606 | ai:codex
    destination = ROOT / ".ci-artifacts"
    if destination.exists():
        shutil.rmtree(destination)
    for pattern in ARTIFACT_PATTERNS:
        for source in (ROOT / "target").glob(pattern):
            if source.is_file():
                source.unlink()


def collect_artifacts():
    destination = ROOT / ".ci-artifacts"
    destination.mkdir(exist_ok=True)
    for pattern in ARTIFACT_PATTERNS:
        for source in (ROOT / "target").glob(pattern):
            if source.is_file():
                relative = source.relative_to(ROOT / "target")
                target = destination / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(source, target)


def main(argv=None, environ=None):
    environ = dict(os.environ if environ is None else environ)
    parser = argparse.ArgumentParser()
    parser.add_argument("--lane", default=environ.get("GDMA_LANE", "gates"))
    parser.add_argument("--collect", action="store_true")
    args = parser.parse_args(argv)
    if args.collect:
        collect_artifacts()
        return 0
    workflow = yaml.safe_load((ROOT / ".github/workflows/ci.yml").read_text())
    job = workflow["jobs"]["build"]
    lanes = job["strategy"]["matrix"]["shard"]
    if args.lane not in lanes:
        raise ValueError("Unknown CI lane")
    # Validate the complete input before any upstream command can have effects.
    for full in (True, False):
        for failed in (True, False):
            validate_job(job, context(args.lane, full, failed, environ))
    clean_artifacts()
    full = True
    base_ref = environ.get("CI_MERGE_REQUEST_TARGET_BRANCH_NAME") or environ.get("CI_DEFAULT_BRANCH", "main")
    if environ.get("CI_PIPELINE_SOURCE") == "merge_request_event":
        full = full_ci(f"origin/{base_ref}")
    else:
        before = environ.get("CI_COMMIT_BEFORE_SHA", "")
        if re.fullmatch(r"[0-9a-f]{40}", before) and before != "0" * 40:
            full = full_ci(before)
    # First/new-branch pushes have no baseline: conservatively run the full suite.
    environment = environ.copy()
    environment["GDMA_LANE"] = args.lane
    environment["GDMA_BASE_REF"] = base_ref
    environment["GDMA_EVENT_NAME"] = event_name(environ)
    (ROOT / "target").mkdir(exist_ok=True)
    environment["GITHUB_STEP_SUMMARY"] = str(ROOT / "target/ci-summary.md")
    environment["RUNNER_TEMP"] = tempfile.mkdtemp(prefix="gdma-runner-", dir=ROOT / "target")
    environment.pop("AIDA_SESSION_ROLE", None)
    initial = context(args.lane, full, environ=environ)
    environment = step_environment(workflow, initial, environment)
    environment = step_environment(job, initial, environment)
    failed = False
    for step in job["steps"]:
        name = step.get("name", step.get("uses", "upstream step"))
        current = context(args.lane, full, failed, environ)
        expression = step.get("if", "true")
        selected = bool(condition(expression, current))
        # GitHub's status functions permit cleanup/reporting after an earlier failure.
        if failed and not re.search(r"\b(?:cancelled|always|success)\s*\(", expression):
            selected = False
        if not selected:
            continue
        if "uses" in step:
            action = step["uses"].split("@", 1)[0]
            if action not in ACTIONS:
                raise ValueError(f"Unmapped upstream action: {action}")
            continue
        if name in INSTALL_STEPS:
            continue
        if "run" not in step:
            raise ValueError(f"Unmapped upstream step: {name}")
        print(f"\n=== {name} ===", flush=True)
        # Step subprocess caps are separate; GitLab enforces the whole-job 40m budget.
        timeout = step.get("timeout-minutes", job.get("timeout-minutes", 40)) * 60
        try:
            command = step_command(step)
            process = subprocess.Popen(
                ["bash", "--noprofile", "--norc", "-e", "-o", "pipefail", "-c", command],
                cwd=ROOT, env=step_environment(step, current, environment),
                stdin=subprocess.DEVNULL, start_new_session=True,
            )
            try:
                returncode = process.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                # Kill descendants before cleanup/collection can observe late writes.
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait()
                raise
            failed = failed or returncode != 0
        except subprocess.TimeoutExpired:
            print(f"Timed out: {name}", file=sys.stderr)
            failed = True
    collect_artifacts()
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
