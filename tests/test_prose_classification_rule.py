#!/usr/bin/env python3
"""Fixtures for the `prose-classification` ratchet rule.

Positive fixtures are the pre-fix code of the specs that were filed for
classifying a decision on another component's message text (BUG-1295,
BUG-1297, BUG-1310, BUG-1435 — which is also BUG-1316's merged fix). Negative
fixtures are legitimate `.contains` uses taken from the same files: a rule
that flags them is too broad and must be narrowed, not allowlisted.

trace:STORY-1382 | ai:claude
"""
import pathlib
import shutil
import subprocess
import tempfile
import textwrap
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
CHECK = ROOT / "scripts" / "check-portability.sh"
RULES = ROOT / "scripts" / "portability-rules.json"
RULE_ID = "prose-classification"
SHELL_RULE_ID = "gnu-only-coreutils"

POSITIVE = {
    # BUG-1295: typed rebase causes recovered from `aida pr rebase` prose.
    "bug_1295": """
        fn rebase(output: Output) {
            let detail = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            if detail.contains("force-push refused")
                || detail.contains("Force-pushing would DROP")
            {
                record("stale-base-refused");
            }
        }
    """,
    # BUG-1297: ambiguous vs not-found split on the error's own text.
    "bug_1297": """
        fn resolve_mailbox_thread(q: &str) -> Result<Id> {
            match resolve(q) {
                Ok(id) => Ok(id),
                Err(error) if error.to_string().contains("ambiguous") => Err(error),
                Err(_) => fallback(q),
            }
        }
    """,
    # BUG-1310: classifiers reading external tool prose (SQLite, OS, glab).
    "bug_1310_sqlite": """
        pub(crate) fn is_database_locked_message(reason: &str) -> bool {
            let lower = reason.to_ascii_lowercase();
            lower.contains("database is locked") || lower.contains("database table is locked")
        }
    """,
    "bug_1310_env": """
        pub(crate) fn is_environmental_failure(message: &str) -> bool {
            let m = message.to_ascii_lowercase();
            m.contains("no space left on device")
                || m.contains("disk full")
        }
    """,
    "bug_1310_glab": """
        fn glab_stderr_is_transient(stderr: &str) -> bool {
            let s = stderr.to_ascii_lowercase();
            s.contains("timeout")
                || s.contains("timed out")
        }
    """,
    # BUG-1316's merged fix == BUG-1435's pre-fix: a supervised hold decided
    # by matching our own producer's refusal sentence.
    "bug_1435": """
        fn classify_drain_merge_failure(pr: u32, error: &anyhow::Error) -> PhaseFailure {
            let detail = format!("{error:#}");
            let hold_prefix = format!("refusing to merge PR-{pr}: supervised merge-hold");
            if detail.contains(&hold_prefix) {
                return PhaseFailure::of(FailureKind::MergeHold, &detail);
            }
            PhaseFailure::new(detail)
        }
    """,
}

NEGATIVE = {
    # forge.rs: parsing our own URL data, not a component's message.
    "url_scheme": """
        fn host(url: &str) -> &str {
            let after_host = if url.contains("://") {
                url
            } else {
                "x"
            };
            after_host
        }
    """,
    "host_match": """
        fn kind(host: &str) -> Kind {
            if host == "github.com" {
                Kind::GitHub
            } else if host == "gitlab.com" || host.contains("gitlab") {
                Kind::GitLab
            } else {
                Kind::Other
            }
        }
    """,
    # mailbox_cmd.rs: membership in our own collection.
    "collection_membership": """
        fn check(known: &[String], agent: &str) {
            if !known.contains(&agent.trim().to_lowercase()) {
                warn();
            }
        }
    """,
    # auto_complete.rs: tag vocabulary check on our own data.
    "tag_vocabulary": """
        fn unknown(t: &str) -> bool {
            t.starts_with("lifecycle:") && !RECOGNIZED_LIFECYCLE_TAGS.contains(&t.as_str())
        }
    """,
    # digest.rs shape: content heuristics over a commit subject, not an error.
    "subject_heuristic": """
        fn bucket(subject: &str) -> Bucket {
            let lower = subject.to_lowercase();
            if lower.contains("skill") || lower.contains("slash command") {
                return Bucket::Skill;
            }
            Bucket::Other
        }
    """,
    # pr_rebase.rs shape: a framing byte, not prose.
    "nul_framing": """
        fn split(stdout: &str) -> Vec<&str> {
            let split = if stdout.contains('\\0') { stdout.split('\\0') } else { stdout.split('\\n') };
            split.collect()
        }
    """,
    # A declared external-tool contract (BUG-1310 inventory) is exempt.
    "declared_external_contract": """
        fn is_network_error(stderr: &str) -> bool {
            // external-prose-classifier: fixture::is_network_error
            let s = stderr.to_ascii_lowercase();
            s.contains("timeout")
                || s.contains("could not resolve")
        }
    """,
    # The matched line is never its own context (review finding 1).
    "self_context_only": """
        fn scan(item: &Item) -> bool {
            let message = item.body();
            if message.contains("TODO") {
                return true;
            }
            false
        }
    """,
    # A bare `e` is not an error signal: `e: &str` is common (review finding 2).
    "bare_e_str_param": """
        fn edge(e: &str) -> bool {
            if e.contains("->") {
                return true;
            }
            false
        }
    """,
    # `// prose-ok: <why>` on the line or the line above (review finding 3).
    "prose_ok_line_above": """
        fn is_locked(reason: &str) -> bool {
            let lower = reason.to_ascii_lowercase();
            // prose-ok: SQLite has no typed busy code on this path
            if lower.contains("database is locked") {
                return true;
            }
            false
        }
    """,
    "prose_ok_same_line": """
        fn is_locked(reason: &str) -> bool {
            let lower = reason.to_ascii_lowercase();
            if lower.contains("database is locked") { // prose-ok: SQLite text only
                return true;
            }
            false
        }
    """,
    # Test assertions on message text are how tests pin wording.
    "test_assertion": """
        #[cfg(test)]
        mod tests {
            #[test]
            fn refuses() {
                let error = run().unwrap_err().to_string();
                if error.contains("ambiguous") {
                    return;
                }
                assert!(error.contains("ambiguous"), "{error}");
            }
        }
    """,
}


def findings(sources: dict[str, str], *, git_repo: bool = True,
             raw: bool = False) -> list[str]:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp).resolve()
        (root / "scripts").mkdir()
        shutil.copy(RULES, root / "scripts" / "portability-rules.json")
        (root / "scripts" / "portability-allowlist.txt").write_text("")
        src = root / "src"
        src.mkdir()
        for name, body in sources.items():
            (src / f"{name}.rs").write_text(textwrap.dedent(body).lstrip())
        if git_repo:
            subprocess.run(["git", "init", "-q"], cwd=root, check=True)
            subprocess.run(["git", "config", "user.email", "fixture@example.invalid"], cwd=root, check=True)
            subprocess.run(["git", "config", "user.name", "Fixture"], cwd=root, check=True)
            subprocess.run(["git", "add", "--", *(f"src/{name}.rs" for name in sources)], cwd=root, check=True)
        result = subprocess.run(
            ["bash", str(CHECK), "--print-findings"],
            cwd=root,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            env={"PATH": "/usr/bin:/bin", "GIT_CEILING_DIRECTORIES": str(root),
                 "HOME": tmp},
        )
        lines = result.stdout.splitlines()
        if raw:
            return lines
        return [line.split("\t", 1)[1] for line in lines if line.startswith(f"{RULE_ID}\t")]


def shell_findings(files: dict[str, str], *, git_repo: bool = True,
                   untracked: dict[str, str] | None = None,
                   rules: list[dict] | None = None,
                   prefix: str = "tests",
                   raw: bool = False) -> tuple[list[str], str]:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp).resolve()
        (root / "scripts").mkdir()
        if rules is None:
            shutil.copy(RULES, root / "scripts" / "portability-rules.json")
        else:
            import json
            (root / "scripts" / "portability-rules.json").write_text(json.dumps(rules))
        (root / "scripts" / "portability-allowlist.txt").write_text("")
        (root / prefix).mkdir(parents=True)
        for name, body in files.items():
            path = root / prefix / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(textwrap.dedent(body).lstrip())
        if untracked:
            for name, body in untracked.items():
                path = root / prefix / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(textwrap.dedent(body).lstrip())
        if git_repo:
            subprocess.run(["git", "init", "-q"], cwd=root, check=True)
            subprocess.run(["git", "config", "user.email", "fixture@example.invalid"], cwd=root, check=True)
            subprocess.run(["git", "config", "user.name", "Fixture"], cwd=root, check=True)
            tracked_paths = [f"{prefix}/{name}" for name in files]
            subprocess.run(["git", "add", "--", *tracked_paths], cwd=root, check=True)
        result = subprocess.run(
            ["bash", str(CHECK), "--print-findings"], cwd=root,
            stdin=subprocess.DEVNULL, capture_output=True, text=True,
            env={"PATH": "/usr/bin:/bin", "GIT_CEILING_DIRECTORIES": str(root), "HOME": tmp},
        )
        lines = result.stdout.splitlines()
        if not raw:
            lines = [line for line in lines if line.startswith(f"{SHELL_RULE_ID}\t")]
        return lines, result.stderr


def markdown_shell_findings(files: dict[str, str], *, mirrors: dict[str, str] | None = None):
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp).resolve()
        (root / "scripts").mkdir()
        shutil.copy(RULES, root / "scripts" / "portability-rules.json")
        (root / "scripts" / "portability-allowlist.txt").write_text("")
        all_files = {**{f"aida-core/templates/{name}": body for name, body in files.items()},
                     **(mirrors or {})}
        for name, body in all_files.items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(textwrap.dedent(body).lstrip())
        subprocess.run(["git", "init", "-q"], cwd=root, check=True)
        subprocess.run(["git", "config", "user.email", "fixture@example.invalid"], cwd=root, check=True)
        subprocess.run(["git", "config", "user.name", "Fixture"], cwd=root, check=True)
        subprocess.run(["git", "add", "--", *all_files], cwd=root, check=True)
        result = subprocess.run(
            ["bash", str(CHECK), "--print-findings"], cwd=root,
            stdin=subprocess.DEVNULL, capture_output=True, text=True,
            env={"PATH": "/usr/bin:/bin", "GIT_CEILING_DIRECTORIES": str(root), "HOME": tmp},
        )
        return [line for line in result.stdout.splitlines()
                if line.startswith(f"{SHELL_RULE_ID}\t")], result.stderr


class ProseClassificationRuleTest(unittest.TestCase):
    def test_each_positive_fixture_matches(self):
        for name, body in POSITIVE.items():
            with self.subTest(name=name):
                self.assertTrue(findings({name: body}), f"{name} was not flagged")

    def test_non_git_fallback_still_scans_rust_fixture(self):
        name, body = next(iter(POSITIVE.items()))
        self.assertTrue(findings({name: body}, git_repo=False), f"{name} was not flagged")

    def test_rust_walk_tolerates_deleted_tracked_file(self):
        surviving_name, body = next(iter(POSITIVE.items()))
        deleted_name = next(name for name in POSITIVE if name != surviving_name)
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp).resolve()
            (root / "scripts").mkdir()
            shutil.copy(RULES, root / "scripts" / "portability-rules.json")
            (root / "scripts" / "portability-allowlist.txt").write_text("")
            src = root / "src"
            src.mkdir()
            (src / f"{surviving_name}.rs").write_text(textwrap.dedent(body).lstrip())
            (src / f"{deleted_name}.rs").write_text(textwrap.dedent(POSITIVE[deleted_name]).lstrip())
            subprocess.run(["git", "init", "-q"], cwd=root, check=True)
            subprocess.run(["git", "config", "user.email", "fixture@example.invalid"], cwd=root, check=True)
            subprocess.run(["git", "config", "user.name", "Fixture"], cwd=root, check=True)
            subprocess.run(["git", "add", "--", f"src/{surviving_name}.rs", f"src/{deleted_name}.rs"], cwd=root, check=True)
            (src / f"{deleted_name}.rs").unlink()
            result = subprocess.run(
                ["bash", str(CHECK), "--print-findings"], cwd=root,
                stdin=subprocess.DEVNULL, capture_output=True, text=True,
                env={"PATH": "/usr/bin:/bin", "GIT_CEILING_DIRECTORIES": str(root), "HOME": tmp},
            )
        lines = result.stdout.splitlines()
        paths = [line.split("\t")[1].split(":", 1)[0] for line in lines if "\t" in line]
        self.assertNotIn("Traceback", result.stderr)
        self.assertTrue(any(path == f"src/{surviving_name}.rs" for path in paths), lines)

    def test_rust_walk_skips_untracked_file(self):
        tracked_name, tracked_body = next(iter(POSITIVE.items()))
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp).resolve()
            (root / "scripts").mkdir()
            shutil.copy(RULES, root / "scripts" / "portability-rules.json")
            (root / "scripts" / "portability-allowlist.txt").write_text("")
            src = root / "src"
            src.mkdir()
            tracked = src / f"{tracked_name}.rs"
            tracked.write_text(textwrap.dedent(tracked_body).lstrip())
            untracked = src / "untracked_positive.rs"
            untracked.write_text(textwrap.dedent(tracked_body).lstrip())
            subprocess.run(["git", "init", "-q"], cwd=root, check=True)
            subprocess.run(["git", "config", "user.email", "fixture@example.invalid"], cwd=root, check=True)
            subprocess.run(["git", "config", "user.name", "Fixture"], cwd=root, check=True)
            subprocess.run(["git", "add", "--", f"src/{tracked_name}.rs"], cwd=root, check=True)
            result = subprocess.run(
                ["bash", str(CHECK), "--print-findings"], cwd=root,
                stdin=subprocess.DEVNULL, capture_output=True, text=True,
                env={"PATH": "/usr/bin:/bin", "GIT_CEILING_DIRECTORIES": str(root), "HOME": tmp},
            )
        lines = result.stdout.splitlines()
        paths = [line.split("\t")[1].split(":", 1)[0] for line in lines if "\t" in line]
        self.assertTrue(any(path == f"src/{tracked_name}.rs" for path in paths), lines)
        self.assertFalse(any("untracked_positive.rs" in path for path in paths), lines)

    def test_rust_walk_skips_nested_linked_worktree(self):
        name, body = next(iter(POSITIVE.items()))
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp).resolve()
            (root / "scripts").mkdir()
            shutil.copy(RULES, root / "scripts" / "portability-rules.json")
            (root / "scripts" / "portability-allowlist.txt").write_text("")
            (root / "src").mkdir()
            (root / "src" / f"{name}.rs").write_text(textwrap.dedent(body).lstrip())
            subprocess.run(["git", "init", "-q"], cwd=root, check=True)
            subprocess.run(["git", "config", "user.email", "fixture@example.invalid"], cwd=root, check=True)
            subprocess.run(["git", "config", "user.name", "Fixture"], cwd=root, check=True)
            subprocess.run(["git", "add", "--", f"src/{name}.rs"], cwd=root, check=True)
            subprocess.run(["git", "commit", "-q", "-m", "fixture"], cwd=root, check=True)
            subprocess.run(["git", "worktree", "add", "-q", ".claude/worktrees/agent-fixture"], cwd=root, check=True)
            nested_git = root / ".claude/worktrees/agent-fixture/.git"
            self.assertTrue(nested_git.is_file())
            result = subprocess.run(
                ["bash", str(CHECK), "--print-findings"], cwd=root,
                stdin=subprocess.DEVNULL, capture_output=True, text=True,
                env={"PATH": "/usr/bin:/bin", "GIT_CEILING_DIRECTORIES": str(root), "HOME": tmp},
            )
        lines = result.stdout.splitlines()
        paths = [line.split("\t")[1].split(":", 1)[0] for line in lines if "\t" in line]
        self.assertTrue(any(path == f"src/{name}.rs" for path in paths), lines)
        self.assertFalse(any(path.startswith(".claude/") for path in paths), lines)

    def test_negative_fixtures_are_not_flagged(self):
        for name, body in NEGATIVE.items():
            with self.subTest(name=name):
                self.assertEqual(findings({name: body}), [])

    def test_prose_ok_needs_a_reason_and_adjacency(self):
        # trace:STORY-1382 | ai:claude — an empty marker, or one two lines up,
        # does not opt out.
        for name, marker in {"empty": "// prose-ok:", "far": "// prose-ok: why\n    let x = 1;"}.items():
            body = f"""
                fn is_locked(reason: &str) -> bool {{
                    let lower = reason.to_ascii_lowercase();
                    {marker}
                    if lower.contains("database is locked") {{
                        return true;
                    }}
                    false
                }}
            """
            with self.subTest(name=name):
                self.assertTrue(findings({name: body}), f"{name} should still be flagged")

    def test_rule_is_production_scoped(self):
        import json

        rule = next(r for r in json.loads(RULES.read_text()) if r["id"] == RULE_ID)
        self.assertEqual(rule["scope"], "production")

    def test_shell_rule_flags_bug_1748_pre_fix_line(self):
        # trace:BUG-1750 | ai:codex
        found, _ = shell_findings({
            "fixture.sh": "expired=$(date -u -d '2 days ago' +%Y-%m-%dT%H:%M:%SZ)\n"
        })
        self.assertTrue(found)

    def test_shell_rule_scans_aida_setup_files(self):
        found, _ = shell_findings(
            {"setup.sh": "count=$(printf '%s\\n' value | grep -oP '\\\\d+')\n"},
            prefix=".aida",
        )
        self.assertEqual(len(found), 1, found)
        self.assertIn(".aida/setup.sh", found[0])

    def test_shell_comments_and_fallback_marker_are_immune(self):
        found, _ = shell_findings({
            "comment.sh": "# expired=$(date -u -d '2 days ago' +%Y-%m-%dT%H:%M:%SZ)\n",
            "fallback.sh": "# portable-fallback: BSD date uses -v for relative dates.\n"
                           "expired=$(date -u -d '2 days ago' +%Y-%m-%dT%H:%M:%SZ)\n",
        })
        self.assertEqual(found, [])

    def test_markdown_shell_fences_scan_all_four_tags_with_real_line_numbers(self):
        body = "\n".join(f"```{tag}\ndate -d yesterday +%s\n```" for tag in
                           ("bash", "sh", "shell", "console"))
        found, stderr = markdown_shell_findings({"tagged.md": body})
        self.assertEqual(len(found), 4, found)
        self.assertTrue(all("aida-core/templates/tagged.md:" in line for line in found), found)
        for line_number in (2, 5, 8, 11):
            self.assertIn(f"aida-core/templates/tagged.md:{line_number}:", stderr)

    def test_markdown_shell_fences_ignore_files_outside_template_prefix(self):
        found, _ = markdown_shell_findings(
            {"inside.md": "```bash\ndate -d yesterday +%s\n```"},
            mirrors={"docs/outside.md": "```bash\ndate -d yesterday +%s\n```"},
        )
        self.assertEqual(len(found), 1, found)
        self.assertIn("aida-core/templates/inside.md", found[0])

    def test_markdown_mirror_reachable_site_is_reported_once_at_master(self):
        content = "```bash\ndate -d yesterday +%s\n```"
        found, _ = markdown_shell_findings(
            {"skills/example/SKILL.md": content},
            mirrors={".claude/skills/example/SKILL.md": content},
        )
        self.assertEqual(len(found), 1, found)
        self.assertIn("aida-core/templates/skills/example/SKILL.md", found[0])
        self.assertNotIn(".claude/", "\n".join(found))

    def test_markdown_fence_comments_and_fallback_markers_are_immune(self):
        found, _ = markdown_shell_findings({"immune.md": """\
            ```bash
            date -d yesterday +%s # portable-fallback: alternate implementation
            # portable-fallback: alternate implementation
            date -d yesterday +%s
            # date -d yesterday +%s
            ```
            """})
        self.assertEqual(found, [], found)

    # Scope isolation must inspect raw output; rule-filtered helpers hide leaks.
    def test_shell_rule_scope_does_not_cross_file_types(self):
        rust_findings = findings({
            "fixture": 'fn f() { let _ = "date -u -d "; }'
        }, raw=True)
        self.assertFalse(any(line.startswith(f"{SHELL_RULE_ID}\t") for line in rust_findings))
        shell, _ = shell_findings({
            "fixture.sh": 'if error.contains("thing") && echo "/tmp"; then\n'
        }, raw=True)
        self.assertFalse(any(line.startswith(("proc-literal\t", "tmp-literal\t",
                                              "prose-classification\t")) for line in shell))

    def test_date_attached_and_equal_forms_are_flagged(self):
        for command in ('date --date="2 days ago"', 'date -d"$iso"'):
            with self.subTest(command=command):
                found, _ = shell_findings({"fixture.sh": f"{command} +%s\n"})
                self.assertEqual(len(found), 1, found)

    def test_attached_stat_and_base64_flags_are_flagged(self):
        for command in ("stat -c%Y file", "base64 -w0 file"):
            with self.subTest(command=command):
                found, _ = shell_findings({"fixture.sh": f"{command}\n"})
                self.assertEqual(len(found), 1, found)

    def test_sed_backup_suffix_remains_portable(self):
        found, _ = shell_findings({"fixture.sh": "sed -i.bak 's/a/b/' file\n"})
        self.assertEqual(found, [])

    def test_git_ls_files_path_and_untracked_files(self):
        found, _ = shell_findings(
            {"tracked.sh": "date -d yesterday +%s\n"},
            untracked={"untracked.sh": "date -d yesterday +%s\n"},
        )
        self.assertEqual(len(found), 1, found)
        self.assertIn("tracked.sh", found[0])
        self.assertNotIn("untracked.sh", "\\n".join(found))

    def test_non_git_fallback_still_scans_shell_fixture(self):
        found, _ = shell_findings(
            {"fixture.sh": "date -d yesterday +%s\n"}, git_repo=False
        )
        self.assertEqual(len(found), 1, found)

    def test_shell_rule_rejects_unsupported_rule_fields(self):
        import json

        rules = json.loads(RULES.read_text())
        shell_rule = next(rule for rule in rules if rule["id"] == SHELL_RULE_ID)
        shell_rule["context"] = {"regex": "x", "before": 1}
        _, stderr = shell_findings({"fixture.sh": "echo ok\n"}, rules=rules)
        self.assertIn(
            "shell-scoped portability rules cannot set context, exempt, or self_evident",
            stderr,
        )


if __name__ == "__main__":
    unittest.main()
