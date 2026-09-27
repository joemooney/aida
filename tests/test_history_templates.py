#!/usr/bin/env python3
"""Isolated CLI/MCP history layout contract, including config mutations.

Run through test_history_templates.sh; uses the existing MCP stdio harness.
trace:STORY-1477 | ai:codex
"""
import json
import os
from pathlib import Path
import pty
import subprocess
import sys
import tempfile

from test_mcp_stdio import McpClient, content_text, parse_spec_id


def main():
    binary = Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory(prefix="aida-history-layout-") as tmp:
        root = Path(tmp)
        env = os.environ.copy()
        env.update(AIDA_OUTPUT_FORMAT="human", AIDA_HISTORY_CACHE="0", NO_COLOR="1")

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
        run("comment", "add", spec, "A comment body which must never be exposed by the layout")
        base = ["history", "--all", "--include-meta", "--limit", "100", "--max-commits", "500"]
        original = run(*base, "--full", "--json").stdout
        full_json = json.loads(original)
        assert full_json["count"] > 0
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
            ["--template","{date:%Q}"], ["--template","{date:%#z}"], ["--template","{id}","events"],
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
        client = McpClient(binary, root, 45)
        try:
            descriptors = client.request("tools/list")["result"]["tools"]
            descriptor = next(d for d in descriptors if d["name"] == "history")
            assert descriptor["inputSchema"]["properties"]["template"]["type"] == "string"
            assert descriptor["inputSchema"]["properties"]["fields"]["type"] == "string"
            def history(args):
                return content_text(client.tool("history",args))
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
        run("history","templates","rm","project:mine")
        assert run(*base,"--full","--json").stdout == original
        print("PASS: history template CLI/config/MCP/TTY/cache contract")


if __name__ == "__main__":
    main()
