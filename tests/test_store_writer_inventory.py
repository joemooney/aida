"""S1a refusal fixtures use disposable source trees; never a live AIDA store."""
# trace:TASK-3222 | ai:codex
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location(
    "store_writer_guard", Path(__file__).resolve().parents[1] / "scripts/check-store-writers.py"
)
guard = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(guard)


def inventory(root, rows):
    text = 'version = 2\nwriter = [\n'
    for key, evidence in sorted(rows.items()):
        row = dict(id=key, lock="fixture lock", gate="fixture gate",
                   disposition="blocked", blocker="fixture policy unresolved",
                   proposed_operation="fixture", evidence_tests=["fixture"], **evidence)
        text += '  { ' + ', '.join(f'{key} = {json.dumps(value)}' for key, value in row.items()) + ' },\n'
    text += ']\n'
    target = root / guard.INVENTORY
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(text)


class WriterInventoryTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.put("aida-core/src/writer.rs", '''
            pub fn write_edit_atomically(path: &Path) {
                std::fs::write(path, "body").unwrap();
            }
        ''')
        self.classify()

    def put(self, path, text):
        target = self.root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text)

    def classify(self):
        inventory(self.root, guard.discover(self.root))

    def assert_refused_then_classified(self):
        self.assertTrue(guard.check(self.root))
        self.classify()
        self.assertEqual([], guard.check(self.root))

    def test_unchanged_evidence_passes(self):
        self.assertEqual([], guard.check(self.root))

    def test_raw_fs_writer_mutant(self):
        self.put("aida-cli-lib/src/new_raw.rs", '''
            fn raw(path: &Path) { std::fs::write(path, "raw").unwrap(); }
        ''')
        self.assert_refused_then_classified()

    def test_wrapper_caller_mutant(self):
        self.put("aida-core/src/new_surface.rs", '''
            fn innocent(path: &Path) { crate::writer::write_edit_atomically(path); }
        ''')
        errors = guard.check(self.root)
        self.assertTrue(any("new_surface.rs" in error for error in errors), errors)
        self.assert_refused_then_classified()

    def test_cache_on_read_mutant(self):
        self.put("aida-cli-lib/src/show.rs", '''
            fn show(conn: &Connection) {
                conn.execute("INSERT INTO cache VALUES (1)", []).unwrap();
            }
        ''')
        self.assert_refused_then_classified()

    def test_new_call_inside_existing_record_is_refused(self):
        path = self.root / "aida-core/src/writer.rs"
        path.write_text(path.read_text().replace(".unwrap();", ".unwrap(); std::fs::rename(path, target);"))
        self.assert_refused_then_classified()

    def test_missing_lock_gate_sink_and_disposition_refuse(self):
        path = self.root / guard.INVENTORY
        text = path.read_text()
        for field in ("sink", "lock", "gate", "disposition"):
            with self.subTest(field=field):
                path.write_text(text.replace(field + ' =', 'missing_' + field + ' ='))
                self.assertTrue(guard.check(self.root))
        path.write_text(text)

    def test_deleted_source_and_duplicate_row_refuse(self):
        path = self.root / guard.INVENTORY
        text = path.read_text()
        path.write_text(text.replace(']\n', text.splitlines()[2] + '\n]\n'))
        self.assertTrue(any("duplicate" in e for e in guard.check(self.root)))
        path.write_text(text)
        (self.root / "aida-core/src/writer.rs").unlink()
        self.assertTrue(any("stale" in e for e in guard.check(self.root)))

    def test_mixed_file_keeps_production_after_inline_tests(self):
        self.put("aida-cli-lib/src/mixed.rs", '''
            #[cfg(test)] mod tests {
                fn test_only() { std::fs::write("fixture", "test").unwrap(); }
            }
            fn production() { std::fs::write("store", "raw").unwrap(); }
        ''')
        rows = [r for r in guard.discover(self.root).values() if r["path"] == "aida-cli-lib/src/mixed.rs"]
        self.assertEqual({"production"}, {r["item"] for r in rows})
        self.assert_refused_then_classified()

    def test_comments_literals_and_nested_comments_are_not_calls(self):
        self.put("aida-cli-lib/src/pure.rs", '''
            /* outer /* std::fs::write("x", "y"); */ nested */
            fn pure() { let example = r#"std::fs::write(path, value)"#; let byte = br#"{ std::fs::write( }"#; }
        ''')
        self.assertFalse(any(r["path"] == "aida-cli-lib/src/pure.rs" for r in guard.discover(self.root).values()))
        self.assertEqual([], guard.check(self.root))

    def test_shell_writer_and_python_writer_refuse(self):
        self.put("scripts/new-writer.sh", 'aida comment add TASK-1 "write"\n')
        self.put("scripts/cache-read.py", 'cache_path.write_bytes(data)\n')
        errors = guard.check(self.root)
        self.assertTrue(any("new-writer.sh" in e for e in errors))
        self.assertTrue(any("cache-read.py" in e for e in errors))
        self.assert_refused_then_classified()

    def test_symlink_source_refuses(self):
        (self.root / "aida-core/src/alias.rs").symlink_to("writer.rs")
        with self.assertRaisesRegex(ValueError, "symlink"):
            guard.check(self.root)

    def test_transitive_wrapper_caller_refuses(self):
        self.put("aida-core/src/relay.rs", "fn relay(p: &Path) { crate::writer::write_edit_atomically(p); }")
        self.classify()
        self.put("aida-core/src/caller.rs", "fn command(p: &Path) { crate::relay::relay(p); }")
        self.assert_refused_then_classified()

    def test_new_crate_cannot_escape_discovery(self):
        self.put("aida-future/src/write.rs", "fn future() { std::fs::write(p, data); }")
        self.assert_refused_then_classified()

    def test_cli_returns_nonzero_for_unclassified_mutant(self):
        import subprocess
        import sys
        self.put("aida-cli-lib/src/cli_mutant.rs", 'fn mutate() { std::fs::write(p, data); }')
        result = subprocess.run(
            [sys.executable, str(guard.ROOT / "scripts/check-store-writers.py"), "--root", str(self.root)],
            capture_output=True, text=True, timeout=10,
        )
        self.assertEqual(1, result.returncode)
        self.assertIn("unclassified writer/caller", result.stderr)

    def test_unrelated_edits_do_not_change_sites(self):
        path = self.root / "aida-core/src/writer.rs"
        path.write_text("// whitespace and comments\n" + path.read_text().replace('"body"', '"changed payload"') + "\nfn pure() { let n = 3; }\n")
        self.assertEqual([], guard.check(self.root))

    def test_bare_name_collision_is_not_a_wrapper(self):
        self.put("aida-core/src/actual.rs", "fn join() { std::fs::write(p, data); } fn add() { std::fs::write(p, data); }")
        self.classify()
        self.put("aida-core/src/pure.rs", "fn pure() { path.join(p); x.display(); x.is_empty(); join(); add(); other::join(); }")
        self.assertEqual([], guard.check(self.root))
        self.put("aida-core/src/caller.rs", "fn caller() { crate::actual::add(); crate::actual::join(); }")
        self.assert_refused_then_classified()

    def test_impl_scopes_do_not_collide(self):
        self.put("aida-core/src/impls.rs", "struct A; struct B; impl A { fn save() { std::fs::write(p, d); } } impl B { fn save() { std::fs::write(p, d); } }")
        items = {r["item"] for r in guard.discover(self.root).values() if r["path"].endswith("impls.rs")}
        self.assertEqual({"impl A::save", "impl B::save"}, items)

    def test_qualified_impl_wrapper_is_propagated(self):
        self.put("aida-core/src/writer.rs", "struct Writer; impl Writer { fn add() { std::fs::write(p, d); } }")
        self.classify()
        self.put("aida-core/src/caller.rs", "fn command() { crate::writer::Writer::add(); }")
        self.assert_refused_then_classified()

    def test_generic_impls_keep_type_identity(self):
        self.put("aida-core/src/types.rs", "impl<T> A<T> { fn add() { std::fs::write(p, d); } } impl<T> B<T> { fn add() { std::fs::write(p, d); } }")
        items = {r["item"] for r in guard.discover(self.root).values() if r["path"].endswith("types.rs")}
        self.assertEqual({"impl <T> A<T>::add", "impl <T> B<T>::add"}, items)
        self.classify()
        self.put("aida-core/src/caller.rs", "fn command() { crate::types::A::add(); }")
        self.assert_refused_then_classified()

    def test_declaration_is_not_a_sink(self):
        self.put("aida-core/src/trait.rs", "trait Read { fn save(&self); } fn save() { read(); }")
        self.assertEqual([], guard.check(self.root))

    def test_duplicate_sink_kind_in_same_item_shares_site(self):
        path = self.root / "aida-core/src/writer.rs"
        path.write_text(path.read_text().replace('.unwrap();', '.unwrap(); std::fs::write(other, data);'))
        self.assertEqual([], guard.check(self.root))

    def test_hook_template_mutant(self):
        self.put("aida-core/templates/hooks/new-hook.sh", '# aida edit TASK-1\necho "aida edit TASK-1"\naida comment add TASK-1 "persist"\n')
        self.assert_refused_then_classified()

    def test_hook_findings_and_session_mutants(self):
        for command in ('findings add --title issue', 'session harness-worktree-release'):
            with self.subTest(command=command):
                self.put("aida-core/templates/hooks/new-hook.sh", 'aida ' + command + '\n')
                self.assert_refused_then_classified()

    def test_shell_function_identity_and_null_redirect(self):
        path = "scripts/existing.sh"
        original = 'existing() { echo ignored >/dev/null; aida edit TASK-1; git commit -m old; }\n'
        self.put(path, original)
        self.classify()
        self.assertFalse(any(r['sink'] == '>' for r in guard.discover(self.root).values()))
        self.put(path, original + 'function new_writer { echo x > "$store/f"; aida edit TASK-2; git commit -m new; }\n')
        errors = guard.check(self.root)
        for kind in ('>', 'aida edit', 'git commit'):
            self.assertTrue(any('new_writer|' + kind in e for e in errors), errors)
        self.assert_refused_then_classified()

    def test_shell_function_declaration_forms(self):
        self.put('scripts/functions.sh', '''
            one() { aida edit TASK-1; }
            function two { aida edit TASK-1; }
            function three() { aida edit TASK-1; }
        ''')
        rows = guard.discover(self.root).values()
        self.assertEqual({'one', 'two', 'three'}, {r['item'] for r in rows if r['path'].endswith('functions.sh')})

    def test_python_path_open_modes_mutants(self):
        for mode in ('w', 'a', 'wb', 'r+'):
            with self.subTest(mode=mode):
                # Remove any previous classification for this same site.
                (self.root / 'scripts/path_mode.py').unlink(missing_ok=True)
                self.classify()
                self.put('scripts/path_mode.py', f'def only_writer(p):\n    with p.open({mode!r}) as f:\n        pass\n')
                self.assert_refused_then_classified()

    def test_python_subprocess_and_filesystem_mutants(self):
        for expression in ("subprocess.run(['aida', 'edit', spec])",
                           "subprocess.run(['git', 'add', path])",
                           "subprocess.run(args=['git', 'hash-object', '-w', path])",
                           "os.open('store', os.O_WRONLY | os.O_CREAT)",
                           "os.remove(path)", "path.touch()", "shutil.move(a, b)", "shutil.copy(a, b)"):
            with self.subTest(expression=expression):
                target = self.root / 'scripts/python_writer.py'
                target.unlink(missing_ok=True)
                self.classify()
                self.put('scripts/python_writer.py', 'def only_writer():\n    ' + expression + '\n')
                self.assert_refused_then_classified()

    def test_python_open_reads_are_not_sinks(self):
        self.put('scripts/read.py', 'p.open("r")\np.open()\nopen(path)\nopen(path, "rb")\nio.open(path, "r")\n')
        self.assertEqual([], guard.check(self.root))

    # trace:TASK-3222 | ai:codex
    def test_global_flags_and_variable_executable_mutants(self):
        commands = {
            'git -C .aida-store commit -m change': 'git commit',
            'git -c user.name=fixture --git-dir=.aida-store/.git --work-tree=.aida-store add objects': 'git add',
            'git --git-dir .aida-store/.git --work-tree .aida-store commit': 'git commit',
            'git -C parent -C .aida-store -c user.name=fixture -c user.email=fixture commit': 'git commit',
            'aida --quiet edit X': 'aida edit',
            'aida --json edit X': 'aida edit',
            'aida --file store --format json edit X': 'aida edit',
            '"$AIDA_BIN" add --title issue': 'aida add',
            '$AIDA queue add X': 'aida queue',
            '"${AIDA_BIN}" queue add X': 'aida queue',
            '"$GIT_BIN" -C .aida-store commit': 'git commit',
            'aida done X': 'aida done',
            'aida pull': 'aida pull',
            'aida future-writer X': 'aida future-writer',
        }
        for command, sink in commands.items():
            with self.subTest(command=command):
                target = self.root / 'scripts/argv-mutant.sh'
                target.unlink(missing_ok=True)
                self.classify()
                self.put('scripts/argv-mutant.sh', command + '\n')
                self.assertEqual([('<script>', sink)], list(guard.script_sites(target.name, command)))
                self.assert_refused_then_classified()

    def test_argv_read_only_allowlist_and_unknown_writers(self):
        for verb in ('done', 'del', 'pull', 'push', 'ship', 'init', 'import',
                     'unclaim', 'archive', 'unarchive', 'defer', 'undefer',
                     'assign', 'unassign', 'triage', 'cache', 'regen', 'rel',
                     'spec', 'mandate', 'schedule', 'future-writer'):
            with self.subTest(verb=verb):
                self.assertEqual('aida ' + verb, guard.argv_writer(['aida', verb, 'X']))
        for argv in (['aida', 'help'], ['aida', '--quiet', 'version'],
                     ['git', '-C', '.aida-store', 'status'],
                     ['git', 'hash-object', 'file'], ['aida', '--json']):
            with self.subTest(argv=argv):
                self.assertIsNone(guard.argv_writer(argv))

    def test_python_string_commands_and_global_git_flags_mutants(self):
        for expression in (
            "subprocess.run('aida edit X', shell=True)",
            "subprocess.run(args='aida --json edit X', shell=True)",
            "os.system('aida edit X')",
            "subprocess.run(['git', '-C', d, '-c', 'maintenance.auto=false', '-c', 'gc.auto=0', 'commit'])",
        ):
            with self.subTest(expression=expression):
                target = self.root / 'scripts/string-mutant.py'
                target.unlink(missing_ok=True)
                self.classify()
                self.put('scripts/string-mutant.py', expression + '\n')
                self.assert_refused_then_classified()
        self.assertEqual([], list(guard.script_sites('read.py', "subprocess.run('aida edit X', shell=False)")))

    def test_multi_clone_harness_writer_mutants(self):
        # Real forms from multi-clone-harness.sh:259,429-433,716.
        commands = (
            ('run_in "$CLONE_A" sh -c \'printf "marker: A" >> .aida-store/metadata.yaml \\\n                && git -C .aida-store add metadata.yaml \\\n                && git -C .aida-store commit -q -m "metadata"\' >/dev/null 2>&1', {'git add', 'git commit'}),
            ('out="$( cd "$dir" && HOME="$WORKDIR/home" AIDA_SESSION_ROLE=advisor \\\n                "$AIDA_BIN" add --title "$title" --type "$stype" --status approved 2>&1 || true )"', {'aida add'}),
            ('"$AIDA_BIN" queue add "$spec"', {'aida queue'}),
        )
        for command, expected in commands:
            with self.subTest(command=command):
                target = self.root / 'scripts/harness-mutant.sh'
                target.unlink(missing_ok=True)
                self.classify()
                self.put('scripts/harness-mutant.sh', command + '\n')
                self.assertTrue(expected <= {sink for _, sink in guard.script_sites(target.name, command)})
                self.assert_refused_then_classified()

        root = Path(__file__).resolve().parents[1]
        actual = set(guard.script_sites('scripts/multi-clone-harness.sh',
                                       (root / 'scripts/multi-clone-harness.sh').read_text()))
        self.assertTrue({('case_MU-203', 'git add'), ('case_MU-203', 'git commit'),
                         ('add_spec', 'aida add'), ('case_MU-401', 'aida queue')} <= actual)

    def test_defer_resurface_maps_reviewed_bare_sweep_edge(self):
        self.put('aida-cli-lib/src/defer_resurface.rs', '''
            fn run() { sweep(); }
            fn sweep() {
                backend.update_spec_atomically(req, callback);
                backend.queue_add(entry);
            }
        ''')
        errors = guard.check(self.root)
        self.assertTrue(any('defer_resurface.rs|sweep|' in e for e in errors), errors)
        self.assertIn('operation mapping changed: cli:defer resurface', errors)
        guard.regenerate(self.root)
        self.assertEqual([], guard.check(self.root))
        row = guard.operations(self.root)['cli:defer resurface']
        self.assertEqual({
            'aida-cli-lib/src/defer_resurface.rs|sweep|update_spec_atomically',
            'aida-cli-lib/src/defer_resurface.rs|sweep|queue_add',
        }, set(row['sites']))
        self.assertIn('bare sweep call', row['resolution_note'])

    def test_queue_done_maps_only_concrete_arm(self):
        self.put('aida-cli-lib/src/queue_cmd.rs', '''
            fn handle_queue_command() {
                match cmd {
                    QueueCommand::Add { id } => { std::fs::rename(a, b); },
                    QueueCommand::Done { id } => { std::fs::write(a, b); },
                }
            }
        ''')
        row = guard.operations(self.root)['cli:queue done']
        self.assertIn('arm QueueCommand::Done', row['entrypoints'][0])
        self.assertTrue(any(s.endswith('|fs::write') for s in row['sites']))
        self.assertFalse(any(s.endswith('|fs::rename') for s in row['sites']))

    def test_mcp_queue_inner_receiver_edges_are_explicit(self):
        self.put('aida-cli-lib/src/mcp.rs', '''
            impl Server {
                fn dispatch() { match tool_name { "queue_add" => self.tool_queue_add(args), } }
                fn tool_queue_add() { self.tool_queue_add_inner(args); }
                fn tool_queue_add_inner() { storage.queue_add(entry); }
            }
        ''')
        row = guard.operations(self.root)['mcp:queue_add']
        self.assertTrue(any(s.endswith('|queue_add') for s in row['sites']))
        self.assertIn('inner', row['resolution_note'])

    def test_script_comments_and_strings_are_not_sites(self):
        self.put("scripts/pure.py", '# cache.write_bytes(data)\nx = "cache.write_bytes(data)"\nopen("input", "r")\n')
        self.put("scripts/pure.sh", '# aida edit TASK-1\necho "aida comment add TASK-1"\n')
        self.assertEqual([], guard.check(self.root))

    def test_regenerate_is_idempotent_and_keeps_policy(self):
        guard.regenerate(self.root)
        target = self.root / guard.INVENTORY
        first = target.read_text()
        guard.regenerate(self.root)
        self.assertEqual(first, target.read_text())
        self.assertEqual([], guard.check(self.root))

    def test_operations_include_unresolved_and_resolved_tools(self):
        self.put("aida-cli-lib/src/mcp.rs", '''
            impl Server {
                fn dispatch() { match tool_name {
                    "write" => self.tool_write(args),
                    "unknown" => self.tool_unknown(args),
                } }
                fn tool_write() { std::fs::write(p, d); }
                fn tool_unknown() { opaque.call(); }
            }
        ''')
        ops = guard.operations(self.root)
        self.assertTrue(ops["mcp:write"]["sites"])
        self.assertEqual([], ops["mcp:unknown"]["sites"])
        guard.regenerate(self.root)
        self.assertEqual([], guard.check(self.root))
        self.put("aida-cli-lib/src/mcp.rs", (self.root / "aida-cli-lib/src/mcp.rs").read_text().replace('"unknown"', '"new_tool"'))
        self.assertTrue(any("operation mapping changed" in e for e in guard.check(self.root)))

    def test_compound_cli_operations_are_not_omitted(self):
        self.put("aida-cli-lib/src/queue_cmd_args.rs", '''
            enum QueueCommand {
                Add { id: String },
                #[clap(name = "custom-done")]
                Done { id: String },
                List,
            }
        ''')
        ops = guard.operations(self.root)
        self.assertEqual({"cli:queue add", "cli:queue custom-done", "cli:queue list"}, set(ops))
        self.assertTrue(all(not row["sites"] for row in ops.values()))
        guard.regenerate(self.root)
        self.assertEqual([], guard.check(self.root))

    def test_actual_ci_fast_tier_wiring(self):
        import tomllib
        import yaml
        root = Path(__file__).resolve().parents[1]
        tiers = tomllib.loads((root / "scripts/ci-gate-tiers.toml").read_text())["tiers"]
        steps = yaml.safe_load((root / ".github/workflows/ci.yml").read_text())["jobs"]["build"]["steps"]
        expected = {
            "Store writer inventory ratchet": "python3 scripts/check-store-writers.py",
            "Store writer inventory mutation fixtures": "python3 -m unittest tests/test_store_writer_inventory.py",
        }
        for name, command in expected.items():
            self.assertEqual("fast", tiers[name])
            matches = [s for s in steps if s.get("name") == name]
            self.assertEqual(1, len(matches))
            self.assertEqual(command, matches[0]["run"])

    def test_invalid_schema_refuses(self):
        (self.root / guard.INVENTORY).write_text('version = 999\n')
        with self.assertRaises(ValueError):
            guard.check(self.root)


if __name__ == "__main__":
    unittest.main()
