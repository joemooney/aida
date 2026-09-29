#!/usr/bin/env python3
"""Isolated CLI/MCP history layout contract, including config mutations.

Run through test_history_templates.sh; uses the existing MCP stdio harness.
trace:STORY-1477 | ai:codex
trace:SPEC-442 | ai:codex
"""
import json
from datetime import datetime, timedelta, timezone
from unittest.mock import patch
from zoneinfo import ZoneInfo
import os
from pathlib import Path
import pty
import subprocess
import sys
import tempfile
import tomllib

from test_mcp_stdio import McpClient, content_text, parse_spec_id


def main():
    binary = Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory(prefix="aida-history-layout-") as tmp:
        root = Path(tmp)
        env = os.environ.copy()
        stamp = (datetime.now(timezone.utc) - timedelta(minutes=5)).replace(microsecond=0)
        env.update(AIDA_OUTPUT_FORMAT="human", AIDA_HISTORY_CACHE="0", NO_COLOR="1",
                   TZ="UTC", GIT_AUTHOR_DATE=stamp.isoformat(), GIT_COMMITTER_DATE=stamp.isoformat())

        def run(*args, ok=True, cwd=root, extra=None):
            result = subprocess.run([str(binary), *args], cwd=cwd, env={**env, **(extra or {})}, text=True, capture_output=True, timeout=45)
            assert (result.returncode == 0) == ok, (args, result.stdout, result.stderr)
            return result

        subprocess.run(["git", "init", "-q"], cwd=root, check=True)
        subprocess.run(["git", "config", "user.email", "layout@example.com"], cwd=root, check=True)
        subprocess.run(["git", "config", "user.name", "Layout Test"], cwd=root, check=True)
        run("init", "--no-skills", "--no-hooks")
        created = run("add", "--title", "Before", "--type", "task", "--status", "draft")
        spec = parse_spec_id(created.stdout, "seed")
        run("edit", spec, "--title", "After", "--priority", "high")
        # Give approval filters a real transition for the date parity checks.
        run("edit", spec, "--status", "approved", extra={"AIDA_SESSION_ROLE": "advisor"})
        run("comment", "add", spec, "A comment body which must never be exposed by the layout")
        base = ["history", "--all", "--include-meta", "--limit", "100", "--max-commits", "500"]
        original = run(*base, "--full", "--json").stdout
        full_json = json.loads(original)
        assert full_json["count"] > 0
        for args in [
            ["--json", "events"], ["events", "--json"],
            ["--format", "json", "events"], ["events", "--format", "json"],
        ]:
            assert json.loads(run(*base, *args).stdout) == full_json
        for extra in [[], ["--id", spec]]:
            for alias in ["full", "oneline"]:
                expected = run(*base, *extra, "--" + alias).stdout
                for name in [alias, "builtin:" + alias]:
                    assert run(*base, *extra, "--template", name).stdout == expected
        # Both sides really see a TTY; compare headers, colors, and footers too.
        def tty(args):
            master, slave = pty.openpty()
            tty_env = {k: v for k, v in env.items() if k != "NO_COLOR"}
            tty_env["CLICOLOR_FORCE"] = "1"
            proc = subprocess.Popen([str(binary), *args], cwd=root, env=tty_env, stdin=subprocess.DEVNULL, stdout=slave, stderr=slave)
            os.close(slave)
            out = bytearray()
            while True:
                try:
                    data = os.read(master, 65536)
                    if not data:
                        break
                    out.extend(data)
                except OSError:
                    break
            os.close(master)
            assert proc.wait(timeout=45) == 0
            return bytes(out)
        for extra in [[], ["--id", spec]]:
            for alias in ["full", "oneline"]:
                assert tty(base + extra + ["--"+alias]) == tty(base + extra + ["--template", "builtin:"+alias])
        fields = "id,date,event,title,priority,comment,from,to"
        selected = json.loads(run(*base, "--fields", fields, "--format", "json").stdout)
        assert list(selected["events"][0]) == fields.split(",")
        assert selected["count"] == full_json["count"]
        for fmt in ["human", "toon"]:
            assert "id" in run(*base, "--fields", fields, "--format", fmt).stdout
        template = "{id}|{kind}|{title}|{priority}|{comment}|{from}|{to}"
        inline = run(*base, "--template", template).stdout
        assert len(inline.splitlines()) == full_json["count"]
        assert "A comment body" not in inline
        assert json.loads(run(*base, "--id", spec, "--fields", "kind", "--json").stdout)["count"] >= 4
        # Config scope resolution, output and comment preservation.
        user = Path.home()/".aida/config.toml"
        project = root/".aida/config.toml"
        user.parent.mkdir(exist_ok=True)
        user.write_text('# user marker\n[unrelated]\nkeep = "user" # untouched\n')
        with project.open("a") as f:
            f.write('\n# project marker\n[unrelated]\nkeep = "project" # untouched\n')
        run(*base, "--template", template, "--save-as-template", "mine")
        saved = run(*base, "--template", "{id}", "--save-as-template", "project:mine")
        assert "commit" in saved.stderr and str(project) in saved.stderr
        assert run(*base,"--template","mine").stdout == inline
        assert run(*base,"--template","user:mine").stdout == inline
        assert run(*base,"--template","project:mine").stdout != inline
        listing = run("history","templates").stdout
        assert "project:mine\t{id}\tuser" in listing
        # Unsupported JSON management must refuse before output or mutation.
        before_configs = (user.read_bytes(), project.read_bytes())
        for args in [[], ["rm", "user:mine"], ["rm", "project:mine"]]:
            refused = run("history", "templates", *args, "--format", "json", ok=False)
            diagnostic = refused.stdout + refused.stderr
            assert "has no JSON projection" in diagnostic, (args, diagnostic)
            assert "builtin:full\t" not in diagnostic and "Updated " not in diagnostic
            assert (user.read_bytes(), project.read_bytes()) == before_configs
            parent_json = run("history", "--json", "templates", *args, ok=False)
            diagnostic = parent_json.stdout + parent_json.stderr
            assert "has no JSON projection" in diagnostic, (args, diagnostic)
            assert "builtin:full\t" not in diagnostic and "Updated " not in diagnostic
            assert (user.read_bytes(), project.read_bytes()) == before_configs
        before = user.read_bytes()
        for options in [
            ["--template","{id}","--save-as-template","mine"],
            ["--template","{bad}","--save-as-template","bad"],
            ["--template","{id}","--since","nonsense","--save-as-template","bad"],
            ["--template","{id}","--id","not-a-spec","--save-as-template","bad"],
            ["--template","{id}","--from","draft","--to","draft","--save-as-template","bad"],
            ["--template","{id}","--save-as-template","builtin:mine"],
            ["--template","mine","--save-as-template","copy"],
            ["--template","{id}","--format","toon"],
            ["--fields","id,id"], ["--fields",""], ["--template","unknown"],
            ["--template","{date:%Q}","--save-as-template","bad"], ["--template","{date:%#z}","--save-as-template","bad"],
            ["--template","{date:%H:%M}","--since","nonsense","--save-as-template","bad"], ["--template","{id}","events"],
        ]:
            run(*base,*options,ok=False)
            assert user.read_bytes() == before
        run(*base,"--template","{event}","--save-as-template","mine","--force")
        assert '# user marker' in user.read_text() and '# untouched' in user.read_text()
        run("history","templates","rm","mine",ok=False)
        run("history","templates","rm","builtin:full",ok=False)
        run("history","templates","rm","user:mine")
        assert run(*base,"--template","mine").stdout == run(*base,"--template","project:mine").stdout
        assert '# project marker' in project.read_text() and '# untouched' in project.read_text()
        # First-save inline history regression: exercise the public loader anew on
        # every CLI invocation, including reload, overwrite, and scoped removal.
        for scope, path, other in [("user", user, project), ("project", project, user)]:
            previous = path.read_bytes()
            other_before = other.read_bytes()
            # Root-level inline history, no templates child; retain init fields.
            body = previous.decode()
            start = body.index("[history]")
            # The fixture appends history last; its tables occupy the suffix.
            assert "[unrelated]" in body[:start]
            body = body[:start]
            path.write_text("history = { keep = 1 } # inline marker\n" + body)
            before_inline = path.read_bytes()
            target = scope + ":first"
            saved = run(*base, "--template", template, "--save-as-template", target)
            assert path.read_bytes() != before_inline
            saved_config = tomllib.loads(path.read_text())
            assert saved_config["history"]["keep"] == 1
            assert saved_config["history"]["templates"]["first"] == template
            assert "Updated " in saved.stderr
            if scope == "project":
                assert "commit this tracked config change" in saved.stderr
            assert target + "\t" + template in run("history", "templates").stdout
            assert run(*base, "--template", target).stdout == inline
            persisted = path.read_bytes()
            run(*base, "--template", "{id}", "--save-as-template", target, ok=False)
            assert path.read_bytes() == persisted
            run(*base, "--template", "{id}", "--save-as-template", target, "--force")
            assert run(*base, "--template", target).stdout == run(*base, "--template", "{id}").stdout
            run("history", "templates", "rm", target)
            assert target + "\t" not in run("history", "templates").stdout
            run(*base, "--template", target, ok=False)
            assert "keep = 1" in path.read_text() and "# inline marker" in path.read_text()
            assert "# untouched" in path.read_text() and "[unrelated]" in path.read_text()
            assert other.read_bytes() == other_before
            path.write_bytes(previous)
        print("PASS: R4 user/project inline first-save reload/force/remove and scope preservation", flush=True)
        # A real sibling worktree writes its own tracked config, not the parent.
        subprocess.run(["git","add",".aida/config.toml"],cwd=root,check=True)
        subprocess.run(["git","-c","commit.gpgsign=false","commit","-qm","config"],cwd=root,check=True)
        sibling = root/"sibling"
        subprocess.run(["git","worktree","add","-qb","layout-sibling",str(sibling)],cwd=root,check=True)
        before_project = project.read_bytes()
        run(*base,"--template","{id}","--save-as-template","project:sibling",cwd=sibling)
        assert project.read_bytes() == before_project
        assert 'sibling = "{id}"' in (sibling/".aida/config.toml").read_text()
        # Cache and git-walk use the same values and order.
        cached = json.loads(run(*base,"--fields",fields,"--json",extra={"AIDA_HISTORY_CACHE":"1"}).stdout)
        assert cached["events"] == selected["events"]
        # Stdio MCP schema, default payload stability, projection, and text parity.
        with patch.dict(os.environ, env):
            client = McpClient(binary, root, 45)
        try:
            descriptors = client.request("tools/list")["result"]["tools"]
            descriptor = next(d for d in descriptors if d["name"] == "history")
            assert descriptor["inputSchema"]["properties"]["template"]["type"] == "string"
            assert descriptor["inputSchema"]["properties"]["fields"]["type"] == "string"
            def history(args):
                return content_text(client.tool("history",args))
            # Both descriptor examples must pass the shared parser and CLI/MCP parity.
            for parameter in ["template", "fields"]:
                example = descriptor["inputSchema"]["properties"][parameter]["example"]
                assert isinstance(example, str)
                mcp_example = history({"limit": 100, parameter: example})
                cli_args = ["--" + parameter, example]
                if parameter == "fields":
                    cli_args += ["--json"]
                    mcp_rows = json.loads(mcp_example)
                    cli_rows = json.loads(run(*base, *cli_args).stdout)
                    assert mcp_rows.keys() == cli_rows.keys()
                    assert mcp_rows["count"] == cli_rows["count"]
                    assert mcp_rows["events"] == cli_rows["events"]
                    assert list(mcp_rows["events"][0]) == example.split(",")
                else:
                    assert mcp_example == run(*base, *cli_args).stdout
            # Isolate source selection to compare bytes of the default contract.
            plain = json.loads(history({"limit":100}))
            assert plain["events"] == full_json["events"]
            projected = json.loads(history({"limit":100,"fields":fields}))
            assert projected["events"] == selected["events"]
            assert list(projected["events"][0]) == fields.split(",")
            assert history({"limit":100,"template":template}) == inline
            assert history({"limit":100,"template":"project:mine"}) == run(*base,"--template","project:mine").stdout
            assert history({"limit":100,"template":"mine"}) == run(*base,"--template","mine").stdout
            for filt, flags in [({"opened":True},["--opened"]),({"comments":True},["--comments"]),({"to":"approved","opened":True},["--to","approved","--opened"]),({"from":"draft"},["--from","draft"]),({"spec_id":spec},["--id",spec]),({"author":"nobody"},["--author","nobody"]),({"since":"2000-01-01","until":"2030-01-01"},["--since","2000-01-01","--until","2030-01-01"])]:
                mcp_text = history({"limit":100,"template":template,**filt})
                cli_text = run(*base,*flags,"--template",template).stdout
                assert mcp_text == cli_text, (filt, repr(mcp_text), repr(cli_text))
            for args in [{"template":"{bad}"},{"fields":"id,id"},{"template":"{id}","fields":"id"},{"template":"{id}","events":True},{"fields":[]},{"template":"missing"}]:
                response = client.request("tools/call",{"name":"history","arguments":args})["result"]
                assert response["isError"], response
                assert "invalid_arg" in json.dumps(response), response
        finally:
            client.close()
        # Real git-decoded dates, independent of descriptor examples. All commits
        # have one controlled recent instant; offset zones cover a date boundary.
        limited = base.copy()
        limited[limited.index("--limit") + 1] = "2"
        date_template = "{date:%Y-%m-%d %H:%M %z} {id} {event}"
        acceptance = "{date:%H:%M} {id} {event}"
        crossed_boundary = False
        for zone in ["UTC", "Etc/GMT+12", "Etc/GMT-14"]:
            zone_env = {"TZ": zone}
            local = stamp.astimezone(ZoneInfo(zone))
            crossed_boundary |= local.date() != stamp.date()
            expected_timestamp = local.strftime("%Y-%m-%d %H:%M")
            baseline = json.loads(run(*base, "--json", extra=zone_env).stdout)
            assert baseline.keys() == full_json.keys()
            assert baseline["count"] == full_json["count"] > 0
            for actual, original_row in zip(baseline["events"], full_json["events"]):
                assert actual["timestamp"] == actual["ts"] == expected_timestamp
                assert {k:v for k,v in actual.items() if k not in ["timestamp", "ts"]} == {k:v for k,v in original_row.items() if k not in ["timestamp", "ts"]}
            # Remove only this fixture's history cache before the cold pass.
            for cache in root.rglob("*history-v*.db*"):
                cache.unlink()
            for mode in ["1", "1", "0"]:  # cold cache, warm cache, disabled git walk
                options = {**zone_env, "AIDA_HISTORY_CACHE": mode}
                selected_dates = json.loads(run(*base, "--since", "2h", "--json", extra=options).stdout)
                assert selected_dates["events"] == baseline["events"]
                assert selected_dates["source"] == ("history-cache" if mode == "1" else "git-walk"), selected_dates
                # Exact acceptance command uses default visibility and limits,
                # unlike the explicitly all-inclusive CLI/MCP parity ledger.
                default_rows = json.loads(run("history", "--since", "2h", "--full", "--json", extra=options).stdout)["events"]
                assert default_rows
                expected = "".join(local.strftime("%H:%M") + " " + r["id"] + " " + r["summary"] + "\n" for r in default_rows)
                actual = run("history", "--since", "2h", "--template", acceptance, extra=options).stdout
                assert actual == expected, (zone, mode, actual, expected)
                run(*base, "--template", date_template, "--save-as-template", "user:dates", "--force", extra=options)
                with patch.dict(os.environ, {**env, **options}):
                    dates_client = McpClient(binary, root, 45)
                try:
                    for filt, flags in [({}, []), ({"opened": True}, ["--opened"]), ({"to": "approved"}, ["--to", "approved"]), ({"spec_id": spec}, ["--id", spec])]:
                        rows = json.loads(run(*limited, "--since", "2h", *flags, "--full", "--json", extra=options).stdout)["events"]
                        assert rows, (zone, mode, filt)
                        for layout in [acceptance, "builtin:compact", "builtin:approvals", date_template, "user:dates"]:
                            if layout == "builtin:approvals":
                                wanted = "".join(local.strftime("%Y-%m-%d") + " " + r["id"] + " " + (r["from"] or "") + " -> " + (r["to"] or "") + "\n" for r in rows)
                            else:
                                fmt = "%H:%M" if layout in [acceptance, "builtin:compact"] else "%Y-%m-%d %H:%M %z"
                                wanted = "".join(local.strftime(fmt) + " " + r["id"] + " " + r["summary"] + "\n" for r in rows)
                            cli = run(*limited, "--since", "2h", *flags, "--template", layout, extra=options).stdout
                            assert cli == wanted, (zone, mode, layout, cli, wanted)
                            mcp = content_text(dates_client.tool("history", {"since": "2h", "limit": 2, "template": layout, **filt}))
                            assert mcp == wanted, (zone, mode, layout, mcp, wanted)
                    plain = json.loads(content_text(dates_client.tool("history", {"limit":100})))
                    # TASK-1526 labels a stale-allowed read on both surfaces, but
                    # by different carriers: the CLI writes `cache` into the JSON
                    # document it prints, while MCP puts it on the envelope
                    # (`structuredContent.cache`) plus a trailing note, leaving the
                    # inner document untouched. Compare the documents themselves.
                    # trace:TASK-1526 | ai:claude
                    assert plain == {k: v for k, v in selected_dates.items() if k != "cache"}
                    assert selected_dates["cache"] == {"stale": False}
                finally:
                    dates_client.close()
                for alias in ["full", "oneline"]:
                    assert run(*base, "--" + alias, extra=options).stdout == run(*base, "--template", "builtin:" + alias, extra=options).stdout
            print("PASS: R5 real-feed dates CLI/MCP/aliases cold/warm/disabled cache in " + zone, flush=True)
        assert crossed_boundary
        run("history", "templates", "rm", "user:dates")
        run("history","templates","rm","project:mine")
        assert run(*base,"--full","--json").stdout == original
        # The minute-only public feed cannot identify which DST-fold instant
        # supplied a local wall time. Preserve wall formats; never invent an offset.
        fold_env = {"TZ": "America/New_York", "GIT_AUTHOR_DATE": "2025-11-02T05:30:00Z",
                    "GIT_COMMITTER_DATE": "2025-11-02T05:30:00Z"}
        fold = parse_spec_id(run("add", "--title", "DST fold", "--type", "task", extra=fold_env).stdout, "fold")
        fold_args = base + ["--id", fold]
        assert run(*fold_args, "--template", "{date:%Y-%m-%d %H:%M}", extra=fold_env).stdout == "2025-11-02 01:30\n"
        before_fold_save = user.read_bytes()
        for directive in ["%z", "%s"]:
            error = run(*fold_args, "--template", "{date:" + directive + "}", "--save-as-template", "fold", extra=fold_env, ok=False)
            assert "ambiguous" in error.stderr
            assert user.read_bytes() == before_fold_save
        print("PASS: ambiguous local DST date rendering and failed-save immutability")
        print("PASS: history template CLI/config/MCP/TTY/cache contract")


if __name__ == "__main__":
    main()
