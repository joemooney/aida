#!/usr/bin/env python3
"""Historical code-growth analysis for a git repository using Tokei (TASK-1605).

trace:TASK-1605 | ai:claude

Read-only history analytics.  The script walks the first-parent history of a
ref, picks one commit per calendar period (always ending at the current tip),
exports each sampled tree with ``git archive`` into a private temporary
directory, runs Tokei on it, assigns every tracked file to exactly one
path-based bucket, and writes CSV + JSON + presentation charts.  It never
touches the repository work tree, index, refs, or any user/AIDA state.

Run ``scripts/analyze_code_growth.py --help`` for options and see
``docs/analytics/code-growth/README.md`` for bucket rules, assumptions, and
rerun commands.
"""

from __future__ import annotations

import argparse
import csv
import datetime as dt
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
from collections import defaultdict
from pathlib import Path, PurePosixPath

SCHEMA_VERSION = 1
CADENCES = ("day", "week", "month", "quarter", "year")
TOKEI_ARGS = ("--output", "json", "--no-ignore")

# --------------------------------------------------------------------------
# Buckets and classification
# --------------------------------------------------------------------------

# (id, label, group, description).  Groups: product (charted as production),
# supporting (charted as supporting corpus), artifact (kept in exports and
# reconciliation, excluded from presentation charts).
BUCKETS = [
    ("core", "Core", "product", "aida-core, proto, legacy requirements-core/requirements-manager monolith, aida-crate"),
    ("cli", "CLI", "product", "aida-cli, aida-cli-lib, legacy requirements-cli"),
    ("server", "Server", "product", "aida-server"),
    ("tui", "TUI", "product", "aida-tui"),
    ("desktop_gui", "Desktop GUI", "product", "aida-gui, aida-desktop, legacy requirements-gui"),
    ("web_ui", "Web UI", "product", "aida-web-react, legacy aida-web, shared TypeScript types"),
    ("developer_tooling", "Developer tooling", "product", "aida-generate-types, helper"),
    ("tests_automation", "Tests & automation", "supporting", "tests, fixtures, benchmarks, scripts, CI workflows, Makefile"),
    ("docs", "Documentation", "supporting", "docs/, prose files (.md/.txt/.docx...), root HTML reports, AIDA discipline docs"),
    ("packaging_integration", "Packaging & integration", "supporting", "templates, agent integrations (.claude/.codex/...), plugins, Docker, root manifests and dotfiles"),
    ("uncategorized", "Uncategorized", "supporting", "tracked files matching no rule"),
    ("artifacts", "Generated, vendored & data artifacts", "artifact", "lockfiles, generated/vendored/dist dirs, requirement-store data, databases, binaries"),
]
BUCKET_INFO = {b[0]: {"id": b[0], "label": b[1], "group": b[2], "description": b[3]} for b in BUCKETS}
PRODUCT_BUCKETS = [b[0] for b in BUCKETS if b[2] == "product"]
SUPPORT_BUCKETS = [b[0] for b in BUCKETS if b[2] == "supporting"]
ARTIFACT_BUCKET = "artifacts"

LOCKFILES = {"Cargo.lock", "pnpm-lock.yaml", "package-lock.json", "yarn.lock", "poetry.lock", "Gemfile.lock"}
ARTIFACT_DIRS = {"generated", "vendor", "vendored", "node_modules", "dist", "target"}
ARTIFACT_PREFIXES = ("aida-store/", ".aida-store/", "data/")
ARTIFACT_SUFFIXES = (".min.js", ".min.css", ".map", ".db", ".sqlite", ".wasm", ".log", ".ttf", ".woff", ".woff2",
                     ".png", ".jpg", ".jpeg", ".gif", ".ico", ".lock")
# This tool's own generated exports (exact paths); the authored README beside
# them stays documentation.
GENERATED_EXPORTS = {f"docs/analytics/code-growth/{n}" for n in (
    "code-growth.json", "code-growth.csv", "code-growth-embedded.csv", "delivery-cadence.csv", "rust-test-markers.csv")}
ARTIFACT_NAME_RE = re.compile(r"^(requirements.*|default_requirements|sample_project)\.ya?ml(\.backup)?$|^requirements\.yaml\.backup$")
TEST_DIR_RE = re.compile(r"(^|/)(tests?|__tests__|benches|([^/]*-)?fixtures?)/")
TEST_ROOT_PREFIXES = ("bench/", "scripts/", "ci/", ".github/")
TEST_ROOT_FILES = {".gitlab-ci.yml", "Makefile", "justfile"}
TEST_NAME_RE = re.compile(r"(^test_[^/]*|[^/]*_tests?\.[^/.]+|[^/]*\.(test|spec)\.[^/.]+|tests\.rs)$")
PACKAGING_PREFIXES = ("aida-core/templates/", "templates/", ".claude/", ".claude-plugin/", ".codex/",
                      ".antigravity/", ".gemini/", ".aida/", "plugins/", "docker/", ".cargo/")
PACKAGING_ROOT_FILES = {".mcp.json", "Cargo.toml", "pnpm-workspace.yaml", "package.json", "rust-toolchain.toml",
                        ".dockerignore", ".env.example", ".gitignore", ".gitattributes"}
DOC_SUFFIXES = (".md", ".mdx", ".rst", ".txt", ".adoc", ".docx", ".pdf")
DOC_NAME_RE = re.compile(r"^(LICENSE|COPYING|NOTICE)[^/]*$")
COMPONENT_PREFIXES = [
    ("aida-core/", "core"), ("requirements-core/", "core"), ("requirements-manager/", "core"),
    ("aida-crate/", "core"), ("proto/", "core"),
    ("aida-cli/", "cli"), ("aida-cli-lib/", "cli"), ("requirements-cli/", "cli"),
    ("aida-server/", "server"),
    ("aida-tui/", "tui"),
    ("aida-gui/", "desktop_gui"), ("aida-desktop/", "desktop_gui"), ("requirements-gui/", "desktop_gui"),
    ("aida-web-react/", "web_ui"), ("aida-web/", "web_ui"), ("shared/", "web_ui"),
    ("aida-generate-types/", "developer_tooling"), ("helper/", "developer_tooling"),
]

# Ordered rule table, kept in sync with classify() and surfaced in the JSON /
# README so the precedence is documented in one place.
RULE_DOCS = [
    ("artifacts", "lockfiles; any path segment generated/vendor/vendored/node_modules/dist/target; "
                  "aida-store/, .aida-store/, data/; requirements*.yaml, default_requirements.yaml, sample_project.yaml; "
                  "minified/map/db/wasm/log/font/image files; this tool's own generated exports under docs/analytics/code-growth/"),
    ("tests_automation", "any tests/test/__tests__/benches/*fixtures/ directory; root bench/, scripts/, ci/, .github/; "
                         ".gitlab-ci.yml, Makefile"),
    ("docs", ".aida/discipline/ (before the generic .aida/ packaging rule)"),
    ("packaging_integration", "aida-core/templates/, templates/, .claude/, .claude-plugin/, .codex/, .antigravity/, .gemini/, "
                              ".aida/, plugins/, docker/, .cargo/ (before generic prose rule, so skill/template .md stay integration)"),
    ("docs", "docs/; *.md/.mdx/.rst/.txt/.adoc/.docx/.pdf; LICENSE/COPYING/NOTICE; root-level *.html"),
    ("packaging_integration", "root manifests and dotfiles: Cargo.toml, pnpm-workspace.yaml, package.json, Dockerfile*, "
                              ".dockerignore, .env.example, .gitignore, .gitattributes, .mcp.json"),
    ("tests_automation", "test-named files: test_*, *_test.*, *_tests.*, *.test.*, *.spec.*, tests.rs"),
    ("<component>", "crate/app directory prefixes (see bucket descriptions): core, cli, server, tui, desktop_gui, web_ui, developer_tooling"),
    ("uncategorized", "everything else"),
]


def classify(path: str) -> str:
    """Return the single bucket id for a tracked-file path (first rule wins)."""
    p = path.replace("\\", "/")
    if p.startswith("./"):
        p = p[2:]
    name = PurePosixPath(p).name
    parts = PurePosixPath(p).parts
    low = p.lower()
    # 1. artifacts
    if (name in LOCKFILES or any(seg in ARTIFACT_DIRS for seg in parts[:-1])
            or p.startswith(ARTIFACT_PREFIXES) or p in GENERATED_EXPORTS or ARTIFACT_NAME_RE.match(name)
            or low.endswith(ARTIFACT_SUFFIXES)):
        return "artifacts"
    # 2. directory-based tests/automation
    if TEST_DIR_RE.search(p) or p.startswith(TEST_ROOT_PREFIXES) or p in TEST_ROOT_FILES:
        return "tests_automation"
    # 3. AIDA discipline docs, then integration/packaging directories
    if p.startswith(".aida/discipline/"):
        return "docs"
    if p.startswith(PACKAGING_PREFIXES):
        return "packaging_integration"
    # 4. documentation
    if (p.startswith("docs/") or low.endswith(DOC_SUFFIXES) or DOC_NAME_RE.match(name)
            or (len(parts) == 1 and low.endswith(".html"))):
        return "docs"
    # 5. root manifests / dotfiles
    if len(parts) == 1 and (name in PACKAGING_ROOT_FILES or name.startswith("Dockerfile")):
        return "packaging_integration"
    # 6. test-named files inside components
    if TEST_NAME_RE.match(name):
        return "tests_automation"
    # 7. product components
    for prefix, bucket in COMPONENT_PREFIXES:
        if p.startswith(prefix):
            return bucket
    return "uncategorized"


# --------------------------------------------------------------------------
# Calendar sampling
# --------------------------------------------------------------------------

def period_of(when: dt.datetime, cadence: str) -> tuple[str, dt.date]:
    """Return (period label, last calendar day of the period) for a UTC datetime."""
    d = when.date()
    if cadence == "day":
        return d.isoformat(), d
    if cadence == "week":
        iso = d.isocalendar()
        return f"{iso[0]}-W{iso[1]:02d}", d + dt.timedelta(days=7 - iso[2])
    if cadence == "month":
        nxt = dt.date(d.year + (d.month == 12), d.month % 12 + 1, 1)
        return f"{d.year}-{d.month:02d}", nxt - dt.timedelta(days=1)
    if cadence == "quarter":
        q = (d.month - 1) // 3
        last_month = q * 3 + 3
        nxt = dt.date(d.year + (last_month == 12), last_month % 12 + 1, 1)
        return f"{d.year}-Q{q + 1}", nxt - dt.timedelta(days=1)
    if cadence == "year":
        return str(d.year), dt.date(d.year, 12, 31)
    raise ValueError(f"unknown cadence {cadence!r}")


def parse_commit_date(value: str) -> dt.datetime:
    return dt.datetime.fromtimestamp(int(value), dt.timezone.utc)


def select_samples(commits: list[dict], cadence: str, every: int = 1) -> list[dict]:
    """Pick the last first-parent commit of each calendar period; keep the tip.

    ``commits`` is oldest-first.  ``every`` thins the period list counting
    backward from the tip, so the tip is always retained.
    """
    if every < 1:
        raise ValueError("every must be >= 1")
    last_by_period: dict[str, int] = {}
    ends: dict[str, dt.date] = {}
    for idx, c in enumerate(commits):
        label, end = period_of(c["when"], cadence)
        last_by_period[label] = idx
        ends[label] = end
    if not commits:
        return []
    tip_idx = len(commits) - 1
    ordered = sorted(last_by_period.items(), key=lambda kv: kv[1])
    keep = [(lab, idx) for pos, (lab, idx) in enumerate(ordered) if (len(ordered) - 1 - pos) % every == 0]
    samples = []
    for lab, idx in keep:
        c = dict(commits[idx])
        c["period"] = lab
        c["kind"] = "tip" if idx == tip_idx else "period_end"
        c["partial"] = c["when"].date() < ends[lab]
        samples.append(c)
    if samples[-1]["sha"] != commits[tip_idx]["sha"]:  # tip dated earlier than a prior commit
        c = dict(commits[tip_idx])
        c["period"], _ = period_of(c["when"], cadence)
        c["kind"], c["partial"] = "tip", True
        samples.append(c)
    return samples


# --------------------------------------------------------------------------
# Git access (read-only)
# --------------------------------------------------------------------------

def run(cmd: list[str], cwd: str | None = None, env: dict | None = None, check: bool = True) -> str:
    res = subprocess.run(cmd, cwd=cwd, env=env, capture_output=True, text=True, errors="replace")
    if check and res.returncode != 0:
        raise RuntimeError(f"{' '.join(cmd)} failed ({res.returncode}): {res.stderr.strip()}")
    return res.stdout


def first_parent_commits(repo: str, ref: str) -> list[dict]:
    out = run(["git", "-C", repo, "log", "--first-parent", "--reverse", "--format=%H%x1f%ct%x1f%s", ref, "--"])
    commits = []
    for line in out.splitlines():
        sha, ts, subject = line.split("\x1f", 2)
        commits.append({"sha": sha, "when": parse_commit_date(ts), "subject": subject})
    return commits


def export_tree(repo: str, sha: str, dest: Path) -> int:
    """Extract tracked regular files of ``sha`` into ``dest``; return skipped link count.

    Symlinks and special entries are skipped: Tokei never follows them, and
    skipping keeps links pointing at live user state out of the scratch tree.
    """
    dest.mkdir(parents=True, exist_ok=True)
    root = dest.resolve()
    skipped = 0
    proc = subprocess.Popen(["git", "-C", repo, "archive", "--format=tar", sha], stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE)
    assert proc.stdout is not None
    with tarfile.open(fileobj=proc.stdout, mode="r|") as tar:
        for member in tar:
            if member.name == "pax_global_header":
                continue
            if not member.isreg():
                if not member.isdir():
                    skipped += 1
                continue
            target = (root / member.name).resolve()
            if root not in target.parents:
                raise RuntimeError(f"archive member escapes export root: {member.name}")
            target.parent.mkdir(parents=True, exist_ok=True)
            src = tar.extractfile(member)
            assert src is not None
            with open(target, "wb") as fh:
                shutil.copyfileobj(src, fh)
    proc.stdout.close()
    err = proc.stderr.read().decode(errors="replace") if proc.stderr else ""
    if proc.stderr:
        proc.stderr.close()
    if proc.wait() != 0:
        raise RuntimeError(f"git archive {sha} failed: {err.strip()}")
    return skipped


# --------------------------------------------------------------------------
# Tokei
# --------------------------------------------------------------------------

def private_env(scratch: Path) -> dict:
    """Environment for child tools: private HOME/XDG so no user tokei config applies."""
    env = dict(os.environ)
    home = scratch / "home"
    home.mkdir(parents=True, exist_ok=True)
    for key in ("HOME", "AIDA_HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"):
        env[key] = str(home)
    return env


def run_tokei(tokei: str, tree: Path, env: dict) -> dict:
    out = run([tokei, *TOKEI_ARGS, "."], cwd=str(tree), env=env)
    return json.loads(out)


def tokei_version(tokei: str, env: dict) -> str:
    return run([tokei, "--version"], env=env).strip()


def _stats(s: dict) -> tuple[int, int, int]:
    return s["code"], s["comments"], s["blanks"]


def collect_files(tokei_json: dict) -> tuple[list[dict], list[dict]]:
    """Flatten Tokei JSON into per-file own-language records plus embedded blobs.

    Tokei's own ``Total`` (and each file's own stats) exclude embedded-language
    blobs (e.g. JavaScript inside HTML, a fenced Rust block inside Markdown), so
    own records alone reconcile with a direct Tokei total.  Blobs are returned
    separately and must never be added to the own totals.
    """
    files, embedded = [], []
    for language, entry in tokei_json.items():
        if language == "Total":
            continue
        for rep in entry.get("reports", []):
            path = rep["name"][2:] if rep["name"].startswith("./") else rep["name"]
            code, comments, blanks = _stats(rep["stats"])
            files.append({"path": path, "language": language, "code": code, "comments": comments, "blanks": blanks})
            for emb_lang, st in rep["stats"].get("blobs", {}).items():
                c, m, b = _stats(st)
                embedded.append({"path": path, "container": language, "language": emb_lang,
                                 "code": c, "comments": m, "blanks": b})
    return files, embedded


def aggregate(files: list[dict], embedded: list[dict]) -> tuple[list[dict], list[dict]]:
    rows: dict[tuple[str, str], dict] = {}
    for f in files:
        key = (classify(f["path"]), f["language"])
        r = rows.setdefault(key, {"bucket": key[0], "language": key[1], "code": 0, "comments": 0, "blanks": 0, "files": 0})
        r["code"] += f["code"]; r["comments"] += f["comments"]; r["blanks"] += f["blanks"]; r["files"] += 1
    emb: dict[tuple[str, str, str], dict] = {}
    for e in embedded:
        key = (classify(e["path"]), e["container"], e["language"])
        r = emb.setdefault(key, {"bucket": key[0], "container": key[1], "language": key[2],
                                 "code": 0, "comments": 0, "blanks": 0})
        r["code"] += e["code"]; r["comments"] += e["comments"]; r["blanks"] += e["blanks"]
    out_rows = sorted(rows.values(), key=lambda r: (r["bucket"], r["language"]))
    for r in out_rows:
        r["lines"] = r["code"] + r["comments"] + r["blanks"]
    out_emb = sorted(emb.values(), key=lambda r: (r["bucket"], r["container"], r["language"]))
    for r in out_emb:
        r["lines"] = r["code"] + r["comments"] + r["blanks"]
    return out_rows, out_emb


# --------------------------------------------------------------------------
# Rust test-attribute markers
# --------------------------------------------------------------------------

# Heuristic: count attribute lines that begin a (trimmed) line outside `//`
# comments.  plain  = #[test]
#           async  = #[<path>::test] (tokio::test, async_std::test, actix_rt::test, ...)
#           rstest = #[rstest] / #[rstest::rstest]
# Parametrised cases (#[case]) are not multiplied; block comments, strings and
# cfg_attr(test) are not parsed, so counts are an indicator, not an exact test
# census.
RUST_MARKER_RE = re.compile(r"^\s*#\[\s*(?P<attr>(?:[A-Za-z_][\w]*::)*(?:test|rstest))\s*(?:\]|\()")


def count_rust_markers(text: str) -> dict:
    counts = {"plain": 0, "async": 0, "rstest": 0}
    for line in text.splitlines():
        stripped = line.lstrip()
        if stripped.startswith("//"):
            continue
        m = RUST_MARKER_RE.match(line)
        if not m:
            continue
        attr = m.group("attr")
        last = attr.rsplit("::", 1)[-1]
        if last == "rstest":
            counts["rstest"] += 1
        elif attr == "test":
            counts["plain"] += 1
        else:
            counts["async"] += 1
    return counts


def rust_markers_for_tree(tree: Path, files: list[dict]) -> dict:
    total = {"plain": 0, "async": 0, "rstest": 0}
    by_bucket: dict[str, dict] = defaultdict(lambda: {"plain": 0, "async": 0, "rstest": 0})
    for f in files:
        if f["language"] != "Rust":
            continue
        try:
            text = (tree / f["path"]).read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        c = count_rust_markers(text)
        bucket = classify(f["path"])
        for k, v in c.items():
            total[k] += v
            by_bucket[bucket][k] += v
    total["total"] = sum(total.values())
    art = by_bucket.get(ARTIFACT_BUCKET, {"plain": 0, "async": 0, "rstest": 0})
    charted = {k: total[k] - art[k] for k in ("plain", "async", "rstest")}
    charted["total"] = sum(charted.values())
    return {"total": total, "charted": charted, "by_bucket": {b: dict(v, total=sum(v.values())) for b, v in sorted(by_bucket.items())}}


# --------------------------------------------------------------------------
# Delivery cadence (first-parent commits + subject-inferred PRs)
# --------------------------------------------------------------------------

PR_PATTERNS = [
    re.compile(r"\(#(\d+)\)\s*$"),                 # squash-merge trailer: "title (#123)"
    re.compile(r"^Merge pull request #(\d+)\b"),   # merge-commit: "Merge pull request #123 from ..."
    re.compile(r"\(!(\d+)\)\s*$"),                 # GitLab-style "title (!123)"
    re.compile(r"See merge request \S*!(\d+)"),    # GitLab merge message
]


def parse_prs(subject: str) -> list[int]:
    found = []
    for pat in PR_PATTERNS:
        for m in pat.finditer(subject):
            n = int(m.group(1))
            if n not in found:
                found.append(n)
    return found


def delivery_cadence(commits: list[dict], cadence: str) -> list[dict]:
    """Per-period first-parent commit count and unique subject-inferred PR count."""
    tip_date = commits[-1]["when"].date() if commits else None
    periods: dict[str, dict] = {}
    for c in commits:
        label, end = period_of(c["when"], cadence)
        p = periods.setdefault(label, {"period": label, "commits": 0, "prs": set(), "end": end})
        p["commits"] += 1
        p["prs"].update(parse_prs(c["subject"]))
    out = []
    if periods:
        # First-parent order is not chronological when commits are backdated, so
        # enumerate the finite calendar range between the earliest and latest
        # period actually seen, independent of walk order.  A period is partial
        # when it ends after the date of the actual ref tip (date-as-of
        # semantics: a backdated tip marks later-dated periods partial too).
        first_end = min(p["end"] for p in periods.values())
        last_end = max(p["end"] for p in periods.values())
        end = first_end
        while end <= last_end:
            label, end = period_of(dt.datetime.combine(end, dt.time(12), tzinfo=dt.timezone.utc), cadence)
            p = periods.get(label, {"commits": 0, "prs": set()})
            out.append({"period": label, "commits": p["commits"], "prs": len(p["prs"]),
                        "period_end": end.isoformat(), "partial": bool(tip_date and tip_date < end)})
            end += dt.timedelta(days=1)
    return out


# --------------------------------------------------------------------------
# Analysis driver
# --------------------------------------------------------------------------

def reconcile(direct: dict, rows: list[dict], embedded: list[dict], sha: str, version: str, files: int) -> dict:
    """Compare bucketed totals with Tokei's own per-language totals on the tip."""
    direct_langs = {k: v for k, v in direct.items() if k != "Total"}
    ours: dict[str, dict] = defaultdict(lambda: {"code": 0, "comments": 0, "blanks": 0, "files": 0})
    for r in rows:
        for k in ("code", "comments", "blanks", "files"):
            ours[r["language"]][k] += r[k]
    langs = []
    ok = True
    for lang in sorted(set(direct_langs) | set(ours)):
        d = direct_langs.get(lang, {})
        dv = {"code": d.get("code", 0), "comments": d.get("comments", 0), "blanks": d.get("blanks", 0),
              "files": len(d.get("reports", []))}
        ov = ours.get(lang, {"code": 0, "comments": 0, "blanks": 0, "files": 0})
        match = all(dv[k] == ov[k] for k in dv)
        ok &= match
        langs.append({"language": lang, "direct": dv, "bucketed": dict(ov), "match": match})
    total = direct.get("Total", {})
    t_direct = {"code": total.get("code", 0), "comments": total.get("comments", 0), "blanks": total.get("blanks", 0)}
    t_ours = {k: sum(r[k] for r in rows) for k in ("code", "comments", "blanks")}
    ok &= t_direct == t_ours
    return {"revision": sha, "tokei": version, "tokei_args": list(TOKEI_ARGS), "ok": bool(ok),
            "direct_total": t_direct, "bucketed_total": t_ours, "files": files,
            "embedded_excluded_code": sum(e["code"] for e in embedded),
            "note": "Direct totals come from an independent Tokei run on a second export of the tip; "
                    "embedded-language blobs are excluded on both sides (Tokei Total excludes them).",
            "languages": langs}


def analyze(repo: str, ref: str, cadence: str, every: int, tokei: str, scratch: Path, progress=print) -> dict:
    env = private_env(scratch)
    commits = first_parent_commits(repo, ref)
    if not commits:
        raise RuntimeError(f"no first-parent commits reachable from {ref}")
    samples = select_samples(commits, cadence, every)
    version = tokei_version(tokei, env)
    result_rows, result_emb, sample_meta, rust = [], [], [], []
    tip_files = 0
    tip_rows: list[dict] = []
    tip_emb: list[dict] = []
    for i, s in enumerate(samples, 1):
        tree = scratch / f"snap-{i}"
        skipped = export_tree(repo, s["sha"], tree)
        files, embedded = collect_files(run_tokei(tokei, tree, env))
        rows, emb = aggregate(files, embedded)
        markers = rust_markers_for_tree(tree, files)
        shutil.rmtree(tree, ignore_errors=True)
        date = s["when"].date().isoformat()
        for r in rows:
            result_rows.append({"revision": s["sha"], "date": date, "period": s["period"], **r})
        for r in emb:
            result_emb.append({"revision": s["sha"], "date": date, "period": s["period"], **r})
        meta = {"revision": s["sha"], "date": date, "period": s["period"], "kind": s["kind"], "partial": s["partial"],
                "subject": s["subject"], "files": len(files), "symlinks_skipped": skipped,
                "code": sum(r["code"] for r in rows), "lines": sum(r["lines"] for r in rows)}
        sample_meta.append(meta)
        rust.append({"revision": s["sha"], "date": date, "period": s["period"], **markers["total"],
                     **{f"charted_{k}": v for k, v in markers["charted"].items()},
                     "by_bucket": markers["by_bucket"]})
        progress(f"[{i}/{len(samples)}] {s['period']} {s['sha'][:10]} code={meta['code']}")
        if s["kind"] == "tip":
            tip_files, tip_rows, tip_emb = len(files), rows, emb
    tip = samples[-1]
    direct_tree = scratch / "direct-tip"
    export_tree(repo, tip["sha"], direct_tree)
    direct = run_tokei(tokei, direct_tree, env)
    shutil.rmtree(direct_tree, ignore_errors=True)
    recon = reconcile(direct, tip_rows, tip_emb, tip["sha"], version, tip_files)
    return {
        "schema": SCHEMA_VERSION, "ref": ref, "tip": tip["sha"], "cadence": cadence, "every": every,
        "tokei": version, "tokei_args": list(TOKEI_ARGS),
        "buckets": [BUCKET_INFO[b[0]] for b in BUCKETS],
        "rules": [{"bucket": b, "match": d} for b, d in RULE_DOCS],
        "samples": sample_meta, "rows": result_rows, "embedded_rows": result_emb,
        "delivery": delivery_cadence(commits, cadence), "rust_tests": rust,
        "reconciliation": recon,
        "notes": ["Own-language Tokei stats only; embedded blobs are in embedded_rows and never summed into totals.",
                  "Generated, vendored and data artifacts are retained here but omitted from presentation charts.",
                  "Delivery PR counts are inferred from first-parent commit subjects and are partial by design."],
    }


# --------------------------------------------------------------------------
# Outputs
# --------------------------------------------------------------------------

ROW_FIELDS = ["revision", "date", "period", "bucket", "language", "code", "comments", "blanks", "lines", "files"]
EMB_FIELDS = ["revision", "date", "period", "bucket", "container", "language", "code", "comments", "blanks", "lines"]


def write_csv(path: Path, fields: list[str], rows: list[dict]) -> None:
    with open(path, "w", newline="", encoding="utf-8") as fh:
        w = csv.DictWriter(fh, fieldnames=fields, extrasaction="ignore", lineterminator="\n")
        w.writeheader()
        w.writerows(rows)


def write_outputs(result: dict, out_dir: Path, charts: bool = True) -> list[Path]:
    out_dir.mkdir(parents=True, exist_ok=True)
    written = []
    p = out_dir / "code-growth.json"
    p.write_text(json.dumps(result, indent=1, sort_keys=False) + "\n", encoding="utf-8"); written.append(p)
    p = out_dir / "code-growth.csv"
    write_csv(p, ROW_FIELDS, result["rows"]); written.append(p)
    p = out_dir / "code-growth-embedded.csv"
    write_csv(p, EMB_FIELDS, result["embedded_rows"]); written.append(p)
    p = out_dir / "delivery-cadence.csv"
    write_csv(p, ["period", "period_end", "commits", "prs", "partial"], result["delivery"]); written.append(p)
    p = out_dir / "rust-test-markers.csv"
    write_csv(p, ["revision", "date", "period", "plain", "async", "rstest", "total", "charted_plain", "charted_async", "charted_rstest", "charted_total"], result["rust_tests"]); written.append(p)
    if charts:
        written.extend(render_charts(result, out_dir))
    return written


def bucket_series(result: dict, buckets: list[str], metric: str) -> tuple[list[dt.date], dict[str, list[int]]]:
    dates = [dt.date.fromisoformat(s["date"]) for s in result["samples"]]
    revs = [s["revision"] for s in result["samples"]]
    table: dict[tuple[str, str], int] = defaultdict(int)
    for r in result["rows"]:
        table[(r["revision"], r["bucket"])] += r[metric]
    return dates, {b: [table[(rev, b)] for rev in revs] for b in buckets}


CADENCE_ADJ = {"day": "daily", "week": "weekly", "month": "monthly", "quarter": "quarterly", "year": "yearly"}
PALETTE = ["#2f6db5", "#e07b39", "#3a9d8f", "#9a5fb4", "#d4a72c", "#c8483f", "#6b7b8c", "#8a9a3b"]


def early_zero_pr_note(delivery: list[dict]) -> str:
    """Chart note for a leading run of zero-PR periods (pre-PR-subject history)."""
    run = 0
    while run < len(delivery) and delivery[run]["prs"] == 0:
        run += 1
    if run == 0 or run == len(delivery):
        return ""
    return (f"{delivery[0]['period']} to {delivery[run - 1]['period']} show 0 PRs because mainline subjects of "
            "that era carry no PR marker; that is not a delivery pause. ")


def render_charts(result: dict, out_dir: Path) -> list[Path]:
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    import matplotlib.ticker

    plt.rcParams.update({"font.family": "DejaVu Sans", "axes.spines.top": False, "axes.spines.right": False,
                         "axes.grid": True, "grid.alpha": 0.25, "figure.dpi": 100})
    tip = result["samples"][-1]
    subtitle = (f"First-parent history of {result['ref']}, {CADENCE_ADJ[result['cadence']]} samples through "
                f"{tip['revision'][:10]} ({tip['date']}). Source: Tokei {result['tokei'].split()[1] if ' ' in result['tokei'] else result['tokei']}.")
    excl = "Excludes generated, vendored, and data artifacts (retained in CSV/JSON)."
    written = []

    def finish(fig, ax, title, note, name):
        import textwrap
        fig.suptitle(title, x=0.06, ha="left", fontsize=17, fontweight="bold")
        fig.text(0.06, 0.915, subtitle, fontsize=9.5, color="#555555", ha="left")
        fig.text(0.06, 0.015, "\n".join(textwrap.wrap(note, 150)), fontsize=8.5, color="#555555", ha="left",
                 va="bottom")
        fig.subplots_adjust(left=0.08, right=0.97, top=0.86, bottom=0.2)
        path = out_dir / name
        fig.savefig(path, dpi=160)
        plt.close(fig)
        written.append(path)

    def stacked(buckets, metric, ylabel, title, name, extra_note=""):
        dates, series = bucket_series(result, buckets, metric)
        fig, ax = plt.subplots(figsize=(12, 6.5))
        labels = [BUCKET_INFO[b]["label"] for b in buckets]
        ax.stackplot(dates, [series[b] for b in buckets], labels=labels, colors=PALETTE[:len(buckets)], alpha=0.95)
        total = [sum(series[b][i] for b in buckets) for i in range(len(dates))]
        ax.annotate(f"{total[-1]:,}", (dates[-1], total[-1]), textcoords="offset points", xytext=(-4, 8),
                    ha="right", fontweight="bold")
        ax.set_xticks(dates)
        ax.set_xticklabels([x["period"] for x in result["samples"]], rotation=45, ha="right", fontsize=8)
        ax.set_ylabel(ylabel)
        ax.yaxis.set_major_formatter(matplotlib.ticker.FuncFormatter(lambda v, _: f"{int(v):,}"))
        ax.legend(loc="upper left", frameon=False, fontsize=9, ncol=2)
        finish(fig, ax, title, excl + (extra_note and " " + extra_note) + " Latest total labelled.", name)

    stacked(PRODUCT_BUCKETS, "code", "Lines of code (Tokei, excluding comments and blanks)",
            "Production code growth by component", "production-growth.png")
    stacked(SUPPORT_BUCKETS, "lines", "Total lines (code + comments + blanks)",
            "Supporting corpus growth", "supporting-growth.png",
            extra_note="Legacy requirement data (aida-store/) moved off the code branch and is excluded as an artifact.")

    # delivery cadence
    d = result["delivery"]
    fig, ax = plt.subplots(figsize=(12, 6.5))
    xs = list(range(len(d)))
    ax.bar([x - 0.2 for x in xs], [r["commits"] for r in d], width=0.4, color=PALETTE[0], label="First-parent commits")
    ax.bar([x + 0.2 for x in xs], [r["prs"] for r in d], width=0.4, color=PALETTE[1], label="Unique PRs (subject-inferred)")
    for x, r in zip(xs, d):
        if r["partial"]:
            ax.axvspan(x - 0.5, x + 0.5, color="#999999", alpha=0.15)
            ax.text(x, max(1, r["commits"]), "partial\nperiod", ha="center", va="bottom", fontsize=8, color="#555555")
    ax.set_xticks(xs); ax.set_xticklabels([r["period"] for r in d], rotation=45, ha="right", fontsize=8)
    ax.set_ylabel(f"Count per {result['cadence']}")
    ax.legend(loc="upper left", frameon=False)
    early = early_zero_pr_note(d)
    finish(fig, ax, f"Delivery cadence on {result['ref']}",
           early + "PR counts are inferred from first-parent commit subjects ((#N), 'Merge pull request #N') and undercount "
           "work merged without such a subject. Shaded period is incomplete.", "delivery-cadence.png")

    # rust tests
    rt = result["rust_tests"]
    dates = [dt.date.fromisoformat(r["date"]) for r in rt]
    fig, ax = plt.subplots(figsize=(12, 6.5))
    ax.stackplot(dates, [[r["charted_plain"] for r in rt], [r["charted_async"] for r in rt],
                         [r["charted_rstest"] for r in rt]],
                 labels=["#[test]", "async #[<path>::test]", "#[rstest]"], colors=PALETTE[:3], alpha=0.95)
    ax.annotate(f"{rt[-1]['charted_total']:,}", (dates[-1], rt[-1]["charted_total"]), textcoords="offset points", xytext=(-4, 8),
                ha="right", fontweight="bold")
    ax.set_ylabel("Test attributes in Rust sources (artifacts excluded)")
    ax.yaxis.set_major_formatter(matplotlib.ticker.FuncFormatter(lambda v, _: f"{int(v):,}"))
    ax.legend(loc="upper left", frameon=False)
    finish(fig, ax, "Rust test attribute growth",
           "Excludes generated, vendored, and data artifacts (raw and per-bucket counts retained in CSV/JSON). "
           "Heuristic: counts line-leading #[test], #[<path>::test] and #[rstest] attributes outside // comments; "
           "parameterised #[case] rows are not multiplied.", "rust-test-growth.png")
    return written


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------

def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0],
                                 formatter_class=argparse.ArgumentDefaultsHelpFormatter)
    ap.add_argument("--repo", default=".", help="git repository (read-only)")
    ap.add_argument("--ref", default="main", help="ref whose first-parent history is sampled; its tip is always included")
    ap.add_argument("--cadence", choices=CADENCES, default="month", help="calendar period for sampling")
    ap.add_argument("--every", type=int, default=1, help="keep every Nth period, counting back from the tip")
    ap.add_argument("--out", default="docs/analytics/code-growth", help="output directory")
    ap.add_argument("--tokei", default=shutil.which("tokei") or "tokei", help="tokei executable")
    ap.add_argument("--work-dir", default=None, help="private scratch parent (default: system temp)")
    ap.add_argument("--no-charts", action="store_true", help="skip PNG rendering")
    ap.add_argument("--allow-mismatch", action="store_true", help="exit 0 even if tip reconciliation fails")
    args = ap.parse_args(argv)

    if shutil.which(args.tokei) is None and not Path(args.tokei).exists():
        print(f"error: tokei binary not found ({args.tokei}); install it or pass --tokei", file=sys.stderr)
        return 2
    repo = str(Path(args.repo).resolve())
    scratch = Path(tempfile.mkdtemp(prefix="code-growth-", dir=args.work_dir))
    try:
        result = analyze(repo, args.ref, args.cadence, args.every, args.tokei, scratch,
                         progress=lambda m: print(m, file=sys.stderr))
        for path in write_outputs(result, Path(args.out), charts=not args.no_charts):
            print(path)
    finally:
        shutil.rmtree(scratch, ignore_errors=True)
    rec = result["reconciliation"]
    print(f"reconciliation {'OK' if rec['ok'] else 'MISMATCH'}: tip {rec['revision'][:10]} "
          f"direct code={rec['direct_total']['code']} bucketed code={rec['bucketed_total']['code']}", file=sys.stderr)
    return 0 if rec["ok"] or args.allow_mismatch else 1


if __name__ == "__main__":
    sys.exit(main())
