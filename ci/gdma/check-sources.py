#!/usr/bin/env python3
"""Optional deployment source policy, supplied entirely by CI variables.

trace:STORY-2606 | ai:codex
"""

import json
import os
from pathlib import Path
import subprocess
import sys
import tomllib
from urllib.parse import urlparse


def git_host(url):
    if "://" in url:
        return urlparse(url).hostname
    return url.split(":", 1)[0].rsplit("@", 1)[-1]


def check_remote(url, host):
    if (not host or git_host(url) != host or
            ("://" in url and urlparse(url).scheme not in {"https", "ssh"})):
        raise ValueError("Git source must use the configured GitLab host")


def registry(url):
    parsed = urlparse(url.removeprefix("sparse+"))
    if (parsed.scheme != "https" or not parsed.hostname or parsed.username
            or parsed.password or parsed.query or parsed.fragment):
        raise ValueError("Package source must use a credential-free HTTPS URL")
    return parsed


def check_registry(url, approved, *, artifact=False):
    actual, expected = registry(url), registry(approved)
    root = expected.path.rstrip("/")
    if (actual.netloc != expected.netloc or
            (not artifact and actual.path.rstrip("/") != root) or
            (artifact and not actual.path.startswith(root + "/"))):
        raise ValueError("Package source must use the configured registry")


def check_cargo(config, lock, host, approved):
    sources = config.get("source", {})
    replacement = sources.get("crates-io", {}).get("replace-with")
    check_registry(sources.get(replacement, {}).get("registry", ""), approved)
    for package in lock.get("package", []):
        source = package.get("source", "")
        if source.startswith("git+"):
            check_remote(source.removeprefix("git+"), host)
        elif source.startswith("registry+") and source not in (
                "registry+https://github.com/rust-lang/crates.io-index",
                "registry+https://index.crates.io/"):
            check_registry(source.removeprefix("registry+"), approved)


def check_npm(lock, host, approved):
    for package in lock.get("packages", {}).values():
        source = package.get("resolved", "")
        if source.startswith("git+"):
            check_remote(source.removeprefix("git+"), host)
        elif source and not source.startswith("file:"):
            check_registry(source, approved, artifact=True)


def main():
    try:
        host = os.environ["GDMA_GITLAB_HOST"]
        cargo_registry = os.environ["GDMA_CARGO_REGISTRY"]
        npm_registry = os.environ["GDMA_NPM_REGISTRY"]
        remote = subprocess.check_output(["git", "remote", "get-url", "origin"],
                                         text=True, stderr=subprocess.DEVNULL).strip()
        check_remote(remote, host)
        with (Path(os.environ["CARGO_HOME"]) / "config.toml").open("rb") as source:
            config = tomllib.load(source)
        with Path("Cargo.lock").open("rb") as source:
            check_cargo(config, tomllib.load(source), host, cargo_registry)
        check_registry(os.environ.get("NPM_CONFIG_REGISTRY", ""), npm_registry)
        with Path("aida-web-react/package-lock.json").open() as source:
            check_npm(json.load(source), host, npm_registry)
    except (ValueError, KeyError, OSError, subprocess.CalledProcessError):
        # Source URLs may contain credentials; never echo input or exceptions.
        print("CI source validation failed; check the deployment source policy", file=sys.stderr)
        return 1
    print("Configured GitLab and package sources verified")
    return 0


if __name__ == "__main__":
    sys.exit(main())
