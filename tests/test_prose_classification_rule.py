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


def findings(sources: dict[str, str]) -> list[str]:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        (root / "scripts").mkdir()
        shutil.copy(RULES, root / "scripts" / "portability-rules.json")
        (root / "scripts" / "portability-allowlist.txt").write_text("")
        src = root / "src"
        src.mkdir()
        for name, body in sources.items():
            (src / f"{name}.rs").write_text(textwrap.dedent(body).lstrip())
        result = subprocess.run(
            ["bash", str(CHECK), "--print-findings"],
            cwd=root,
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            env={"PATH": "/usr/bin:/bin", "GIT_CEILING_DIRECTORIES": tmp,
                 "HOME": tmp},
        )
        return [
            line.split("\t", 1)[1]
            for line in result.stdout.splitlines()
            if line.startswith(f"{RULE_ID}\t")
        ]


class ProseClassificationRuleTest(unittest.TestCase):
    def test_each_positive_fixture_matches(self):
        for name, body in POSITIVE.items():
            with self.subTest(name=name):
                self.assertTrue(findings({name: body}), f"{name} was not flagged")

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


if __name__ == "__main__":
    unittest.main()
