"""Host proxy boundary, SQLite backup and installed-CodeGraph compatibility.
# trace:STORY-2630 | ai:codex
"""
import importlib.machinery
import importlib.util
import json
import os
from pathlib import Path
import shutil
import socket
import sqlite3
import subprocess
import tempfile
import threading
import time
import unittest
from unittest import mock

SCRIPT = Path(__file__).resolve().parents[1] / "scripts/codegraph/codegraph"
loader = importlib.machinery.SourceFileLoader("codegraph_proxy", str(SCRIPT))
spec = importlib.util.spec_from_loader(loader.name, loader)
proxy = importlib.util.module_from_spec(spec)
loader.exec_module(proxy)


class ProxyTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix="codegraph-proxy-")
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.primary = self.root / "primary"
        (self.primary / ".codegraph").mkdir(parents=True)
        self.db = self.primary / ".codegraph/codegraph.db"
        self.conn = sqlite3.connect(self.db)
        self.addCleanup(self.conn.close)
        self.conn.executescript("CREATE TABLE files(path TEXT); INSERT INTO files VALUES ('caller.rs');")
        self.state = self.root / "host-state"
        self.state.mkdir()
        self.upstream = self.root / "upstream"
        self.upstream.write_text('''#!/usr/bin/python3
import json, pathlib, sqlite3, sys
p = pathlib.Path(sys.argv[sys.argv.index('--path')+1])
with sqlite3.connect(p / '.codegraph/codegraph.db') as db:
    print(json.dumps({'argv': sys.argv[1:], 'files': db.execute('select path from files').fetchall()}))
''')
        self.upstream.chmod(0o700)

    def host(self):
        host = proxy.Host(self.primary, self.state, self.upstream)
        self.addCleanup(lambda: host.snapshot and host.snapshot.cleanup())
        return host

    def test_failed_descriptor_publication_cleans_temporary_file(self):
        directory = self.primary / ".codegraph"
        descriptor = directory / proxy.DESCRIPTOR
        before = set(directory.iterdir())
        for operation in ("json.dump", "os.replace"):
            with self.subTest(operation=operation):
                with mock.patch.object(proxy.json if operation == "json.dump" else proxy.os,
                                       operation.split(".")[1], side_effect=OSError("disk full")):
                    with self.assertRaisesRegex(OSError, "disk full"):
                        proxy.write_descriptor(directory, descriptor, {"protocol": 1})
                self.assertEqual(set(directory.iterdir()), before)

    def test_refuses_nonqueries_arbitrary_options_and_unbounded_inputs(self):
        for request in ({"op": "sync"}, {"op": "callers", "symbol": "x", "path": "/tmp"},
                        {"op": "callers", "symbol": "--path"}, {"op": "status", "argv": []},
                        {"op": "callers", "symbol": "x", "limit": True},
                        {"op": "impact", "symbol": "x", "depth": 11},
                        {"op": "callers", "symbol": "x" * 257}, []):
            with self.subTest(request=request), self.assertRaises(ValueError):
                proxy.validate(request)
        for op in ("callers", "callees", "impact"):
            proxy.validate({"op": op, "symbol": "module::symbol"})

    def test_snapshot_includes_wal_and_keeps_primary_unchanged(self):
        self.conn.execute("PRAGMA journal_mode=WAL")
        self.conn.execute("INSERT INTO files VALUES ('wal-only.rs')")
        self.conn.commit()
        before = self.db.read_bytes()
        wal_before = Path(str(self.db) + "-wal").read_bytes()
        host = self.host()
        result = host.query({"op": "callers", "symbol": "known"})
        output = json.loads(result["output"])
        self.assertEqual(result["rc"], 0)
        self.assertEqual(output["files"], [["caller.rs"], ["wal-only.rs"]])
        snapshot = Path(host.snapshot.name)
        self.assertIn(str(snapshot), output["argv"])
        self.assertNotIn(str(self.primary), output["argv"])
        self.assertEqual(self.db.read_bytes(), before)
        self.assertEqual(Path(str(self.db) + "-wal").read_bytes(), wal_before)
        self.conn.execute("INSERT INTO files VALUES ('next.rs')")
        self.conn.commit()
        host.refresh()
        self.assertNotEqual(Path(host.snapshot.name), snapshot)
        self.assertFalse(snapshot.exists())
        self.assertEqual(len(json.loads(host.query({"op": "callees", "symbol": "known"})["output"])["files"]), 3)

    def test_old_snapshot_cleanup_failure_keeps_published_generation_queryable(self):
        host = self.host()
        host.refresh()
        previous = host.snapshot
        self.conn.execute("INSERT INTO files VALUES ('new.rs')")
        self.conn.commit()
        with mock.patch.object(previous, "cleanup", side_effect=OSError("cleanup denied")):
            host.refresh()
        self.assertNotEqual(host.snapshot.name, previous.name)
        self.assertTrue(Path(host.snapshot.name).exists())
        result = host.query({"op": "callers", "symbol": "known"})
        self.assertEqual(result["rc"], 0)
        self.assertIn(["new.rs"], json.loads(result["output"])["files"])
        previous.cleanup()

    def test_socket_recovery_preserves_live_service_and_non_socket_paths(self):
        path = self.root / "recovery.sock"
        with socket.socket(socket.AF_UNIX) as live:
            live.bind(str(path))
            live.listen(1)
            inode = path.stat().st_ino
            with socket.socket(socket.AF_UNIX) as candidate:
                with self.assertRaisesRegex(ValueError, "already running"):
                    proxy.bind_socket(candidate, path)
            self.assertEqual(path.stat().st_ino, inode)
        # A killed server leaves exactly this refused socket behind.
        with socket.socket(socket.AF_UNIX) as restarted:
            proxy.bind_socket(restarted, path)
            restarted.listen(1)
            with socket.socket(socket.AF_UNIX) as client:
                client.connect(str(path))
        path.unlink()
        for kind in ("file", "symlink"):
            with self.subTest(kind=kind):
                if kind == "file":
                    path.write_text("preserve")
                else:
                    path.symlink_to(self.db)
                with socket.socket(socket.AF_UNIX) as candidate:
                    with self.assertRaisesRegex(ValueError, "not a socket"):
                        proxy.bind_socket(candidate, path)
                self.assertTrue(path.is_symlink() if kind == "symlink" else path.read_text() == "preserve")
                path.unlink()

    def test_old_source_age_is_not_reset_by_new_snapshot(self):
        old = time.time() - 7200
        os.utime(self.db, (old, old))
        # An empty WAL created by an observer is not fresh indexing evidence.
        Path(str(self.db) + "-wal").touch()
        host = self.host()
        host.refresh()
        self.assertGreaterEqual(host.status()["snapshot_age_seconds"], 7200)
        self.assertLess(time.time() - host.captured_at, 2)

    def test_reindex_lock_serves_previous_snapshot_with_refresh_error(self):
        host = self.host()
        host.refresh()
        previous = host.snapshot.name
        lock = self.primary / ".codegraph/codegraph.lock"
        lock.write_text("host indexer")
        result = host.query({"op": "callers", "symbol": "known"})
        self.assertEqual(result["rc"], 0)
        self.assertIn("reindex lock", result["refresh_error"])
        self.assertEqual(json.loads(result["output"])["files"], [["caller.rs"]])
        self.assertEqual(host.snapshot.name, previous)
        self.assertEqual(host.query({"op": "status"})["rc"], 0)
        lock.unlink()
        self.assertEqual(host.query({"op": "status"})["rc"], 0)

    def test_query_deadline_and_output_limit(self):
        for code in ("import time; time.sleep(10)", "print('x' * 2000)"):
            self.upstream.write_text("#!/usr/bin/python3\n" + code + "\n")
            host = self.host()
            old_time, old_output = proxy.QUERY_TIMEOUT, proxy.MAX_OUTPUT
            proxy.QUERY_TIMEOUT, proxy.MAX_OUTPUT = 0.1, 1000
            try:
                with self.assertRaises((TimeoutError, ValueError)):
                    host.query({"op": "callers", "symbol": "known"})
            finally:
                proxy.QUERY_TIMEOUT, proxy.MAX_OUTPUT = old_time, old_output

    def test_fast_exit_still_cleans_query_descendants(self):
        marker = self.state / "survived"
        child = "import pathlib,time; time.sleep(0.2); pathlib.Path(" + repr(str(marker)) + ").touch()"
        self.upstream.write_text("#!/usr/bin/python3\nimport subprocess\nsubprocess.Popen(['/usr/bin/python3', '-c', " + repr(child) + "])\nprint('{}')\n")
        self.assertEqual(self.host().query({"op": "callers", "symbol": "known"})["rc"], 0)
        time.sleep(0.3)
        self.assertFalse(marker.exists(), "fast-exiting query left a running descendant")

    def test_socket_protocol_and_seat_discovery(self):
        path = str(self.root / "query.sock")
        server = socket.socket(socket.AF_UNIX)
        self.addCleanup(server.close)
        server.bind(path)
        server.listen(1)
        server.settimeout(2)
        def respond():
            conn, _ = server.accept()
            with conn:
                self.assertEqual(proxy.receive(conn, proxy.MAX_REQUEST), {"op": "status"})
                proxy.send(conn, {"rc": 0, "snapshot_age_seconds": 60})
        thread = threading.Thread(target=respond)
        thread.start()
        self.assertEqual(proxy.client({"op": "status"}, path)["snapshot_age_seconds"], 60)
        thread.join(2)
        self.assertFalse(thread.is_alive())
        seat = self.root / "seat"
        seat.mkdir()
        (seat / ".codegraph").symlink_to(self.primary / ".codegraph", target_is_directory=True)
        (self.primary / ".codegraph" / proxy.DESCRIPTOR).write_text(json.dumps({"protocol": 1, "socket": "/untrusted"}))
        # Derive the socket from the shared directory, never from arbitrary JSON.
        self.assertEqual(proxy.discover(seat), str(self.primary / ".codegraph/lane-query.sock"))

    def test_discovery_refuses_non_object_descriptor(self):
        for body in ("[]", "null", "1", '"text"'):
            with self.subTest(body=body):
                (self.primary / ".codegraph" / proxy.DESCRIPTOR).write_text(body)
                result = subprocess.run([str(SCRIPT), "status", "--path", str(self.primary)],
                                        capture_output=True, text=True, timeout=5)
                self.assertEqual(result.returncode, 1)
                self.assertIn("unsupported codegraph proxy protocol", result.stderr)
                self.assertNotIn("Traceback", result.stderr)

    def test_status_and_heartbeat_stay_responsive_during_query_and_stalled_peer(self):
        marker = self.state / "query-started"
        self.upstream.write_text("#!/usr/bin/python3\nimport pathlib,time\npathlib.Path(" +
                                 repr(str(marker)) + ").touch()\ntime.sleep(1)\nprint('{}')\n")
        server = subprocess.Popen([str(SCRIPT), "serve", "--primary", str(self.primary),
                                   "--state", str(self.state), "--upstream", str(self.upstream)],
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        query_result = []
        query_errors = []
        query = None
        stalled = socket.socket(socket.AF_UNIX)
        try:
            path = str(self.primary / ".codegraph/lane-query.sock")
            deadline = time.monotonic() + 5
            while not Path(path).exists():
                if server.poll() is not None or time.monotonic() > deadline:
                    self.fail("proxy did not start")
                time.sleep(0.01)
            stalled.connect(path)
            stalled.sendall(b"{")
            def run_query():
                try:
                    query_result.append(proxy.client({"op": "callers", "symbol": "known"}, path))
                except Exception as exc:
                    query_errors.append(exc)
            query = threading.Thread(target=run_query)
            query.start()
            while not marker.exists():
                if time.monotonic() > deadline:
                    self.fail("query did not start")
                time.sleep(0.01)
            for _ in range(3):
                started = time.monotonic()
                self.assertEqual(proxy.client({"op": "status"}, path)["rc"], 0)
                self.assertLess(time.monotonic() - started, 0.25)
                descriptor = json.loads((self.primary / ".codegraph" / proxy.DESCRIPTOR).read_text())
                self.assertLess(time.time() - descriptor["observed_at"], 0.5)
                time.sleep(0.1)
            query.join(5)
            self.assertFalse(query.is_alive())
            self.assertEqual(query_errors, [])
            self.assertEqual(query_result[0]["rc"], 0)
        finally:
            stalled.close()
            server.terminate()
            try:
                server.communicate(timeout=8)
            except subprocess.TimeoutExpired:
                server.kill()
                server.communicate()
            if query is not None:
                query.join(5)
        self.assertEqual(server.returncode, 0)

    def test_protocol_refuses_oversized_and_truncated_requests(self):
        for data in (b"x" * 100, b'{"op":"status"}'):
            left, right = socket.socketpair()
            with left, right:
                left.sendall(data)
                left.shutdown(socket.SHUT_WR)
                with self.assertRaises(ValueError):
                    proxy.receive(right, 50)

    def test_protocol_refuses_excessive_nesting_and_times_out_incomplete_peer(self):
        left, right = socket.socketpair()
        with left, right:
            left.sendall(b"[" * 1500 + b"]" * 1500 + b"\n")
            with self.assertRaises(ValueError):
                proxy.validate(proxy.receive(right, proxy.MAX_REQUEST))
        left, right = socket.socketpair()
        with left, right:
            left.sendall(b"{")
            with self.assertRaises(TimeoutError):
                proxy.receive(right, proxy.MAX_REQUEST, timeout=0.05)

    def test_service_query_from_read_only_seat_and_graceful_restart(self):
        bwrap = shutil.which("bwrap")
        if not bwrap:
            self.skipTest("bubblewrap is not installed")
        probe = subprocess.run([bwrap, "--ro-bind", "/", "/", "--unshare-net", "--", "/usr/bin/true"],
                               capture_output=True, timeout=5)
        if probe.returncode:
            self.skipTest("nested bubblewrap unavailable: " + probe.stderr.decode().strip())
        seat = self.root / "seat"
        seat.mkdir()
        (seat / ".codegraph").symlink_to(self.primary / ".codegraph", target_is_directory=True)
        for _ in range(2):
            server = subprocess.Popen([str(SCRIPT), "serve", "--primary", str(self.primary),
                                       "--state", str(self.state), "--upstream", str(self.upstream)],
                                      stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            try:
                deadline = time.monotonic() + 5
                descriptor = self.primary / ".codegraph" / proxy.DESCRIPTOR
                while not descriptor.exists() or not (descriptor.parent / "lane-query.sock").exists():
                    if server.poll() is not None or time.monotonic() > deadline:
                        self.fail("proxy did not start")
                    time.sleep(0.02)
                command = [bwrap, "--ro-bind", "/", "/", "--unshare-net", "--bind", str(seat), str(seat),
                           "--chdir", str(seat), "--", "/usr/bin/python3", "-c", '''
import errno, os, subprocess, sys
for p in sys.argv[1:3]:
    try:
        fd = os.open(p, os.O_RDWR | os.O_CREAT, 0o600)
    except OSError as e:
        assert e.errno in (errno.EROFS, errno.EACCES), e
    else:
        os.close(fd)
        raise AssertionError('index was writable')
sys.exit(subprocess.call([sys.argv[3], 'callers', 'known', '--json']))
''', str(self.db), str(self.state / "denied-write"), str(SCRIPT)]
                result = subprocess.run(command, capture_output=True, text=True, timeout=20)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(json.loads(result.stdout)["files"], [["caller.rs"]])
            finally:
                server.terminate()
                try:
                    server.communicate(timeout=5)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.communicate()
            self.assertEqual(server.returncode, 0)
            self.assertFalse((self.primary / ".codegraph/lane-query.sock").exists())

    def test_installed_codegraph_returns_real_edges_from_host_snapshot(self):
        binary = shutil.which("codegraph")
        if not binary:
            self.skipTest("bundled CodeGraph 1.6.0 not installed")
        upstream = Path(binary).resolve()
        schema = upstream.parent.parent / "lib/dist/db/schema.sql"
        if not schema.is_file():
            self.skipTest("bundled CodeGraph schema not available")
        self.conn.close()
        self.db.unlink()
        conn = sqlite3.connect(self.db)
        conn.executescript(schema.read_text())
        conn.execute("INSERT OR REPLACE INTO schema_versions VALUES (9, 1, 'test schema')")
        for ident, name, line in (("target", "known_target", 1), ("caller", "known_caller", 5)):
            conn.execute("INSERT INTO nodes(id,kind,name,qualified_name,file_path,language,start_line,end_line,start_column,end_column,updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
                         (ident, "function", name, name, "fixture.rs", "rust", line, line + 2, 0, 1, 1))
        conn.execute("INSERT INTO edges(source,target,kind) VALUES ('caller','target','calls')")
        conn.commit()
        conn.close()
        before = self.db.read_bytes()
        host = proxy.Host(self.primary, self.state, upstream)
        self.addCleanup(lambda: host.snapshot and host.snapshot.cleanup())
        for op, symbol, key, expected in (("callers", "known_target", "callers", "known_caller"),
                                           ("callees", "known_caller", "callees", "known_target"),
                                           ("impact", "known_target", "affected", "known_caller")):
            with self.subTest(op=op):
                result = host.query({"op": op, "symbol": symbol})
                self.assertEqual(result["rc"], 0, result["output"])
                self.assertIn(expected, [node["name"] for node in json.loads(result["output"])[key]])
        self.assertEqual(self.db.read_bytes(), before)


if __name__ == "__main__":
    unittest.main()
