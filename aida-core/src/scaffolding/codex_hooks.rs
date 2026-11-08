//! Compatibility exports for the profile-owned hook backends.
// trace:STORY-2539 | ai:codex
pub(crate) use crate::vendors::hooks::handler_raw_body;
pub use crate::vendors::hooks::{
    antigravity_handler_names, antigravity_hooks_wired, codex_hooks_wired, handler_names,
    scaffold_antigravity_hooks, scaffold_codex_hooks, CodexHooksOutcome, AGY_EXPIRY_NOTICE_KEY,
    AGY_HOOKS_DIR_REL, AGY_HOOKS_JSON_REL, HOOKS_DIR_REL, HOOKS_JSON_REL,
};

#[cfg(test)]
use crate::vendors::hooks::set_executable;

#[cfg(test)]
mod tests {
    use super::*;

    /// The wiring must cover exactly the events `settings.json` wires, with the
    /// same matchers and the same handlers. Without this, editing the Claude
    /// settings silently leaves Codex behind — the drift this whole design is
    /// trying to make impossible.
    #[test]
    fn codex_hooks_match_settings() {
        let settings: serde_json::Value = serde_json::from_str(
            crate::templates::EMBEDDED_TEMPLATES
                .get("settings.json")
                .expect("settings.json embedded"),
        )
        .expect("settings.json parses");
        let codex: serde_json::Value = serde_json::from_str(
            crate::templates::EMBEDDED_TEMPLATES
                .get(
                    crate::vendors::profile(crate::vendors::LaunchVendor::Codex)
                        .hooks
                        .template,
                )
                .expect("codex-hooks.json embedded"),
        )
        .expect("codex-hooks.json parses");

        let sh = settings.get("hooks").and_then(|h| h.as_object()).unwrap();
        let ch = codex.get("hooks").and_then(|h| h.as_object()).unwrap();

        // trace:TASK-1744 | ai:antigravity
        assert!(codex.to_string().contains("aida-grant-expiry-notice.sh"));
        assert!(codex.to_string().contains("--client codex"));
        assert!(handler_names()
            .iter()
            .any(|name| name == "aida-grant-expiry-notice.sh"));
        let tmp = tempfile::tempdir().unwrap();
        scaffold_codex_hooks(tmp.path(), true).unwrap();
        assert!(tmp
            .path()
            .join(HOOKS_DIR_REL)
            .join("aida-grant-expiry-notice.sh")
            .is_file());
        scaffold_antigravity_hooks(tmp.path(), true).unwrap();
        assert!(tmp
            .path()
            .join(AGY_HOOKS_DIR_REL)
            .join("aida-grant-expiry-notice.sh")
            .is_file());
        assert!(tmp.path().join(AGY_HOOKS_JSON_REL).is_file());
        assert!(antigravity_hooks_wired(tmp.path()));

        // Every Claude event Codex supports must be present.
        const CODEX_EVENTS: &[&str] = &[
            "PreToolUse",
            "PermissionRequest",
            "PostToolUse",
            "PreCompact",
            "PostCompact",
            "UserPromptSubmit",
            "SessionStart",
            "Stop",
            "SubagentStart",
            "SubagentStop",
        ];
        for ev in sh.keys() {
            if !CODEX_EVENTS.contains(&ev.as_str()) {
                continue; // no Codex equivalent; legitimately dropped
            }
            assert!(
                ch.contains_key(ev),
                "settings.json wires `{ev}` but the Codex wiring does not — \
                 regenerate templates/codex-hooks.json"
            );
            let s_entries = sh[ev].as_array().unwrap();
            let c_entries = ch[ev].as_array().unwrap();
            assert_eq!(
                s_entries.len(),
                c_entries.len(),
                "`{ev}` has {} Claude entries but {} Codex entries",
                s_entries.len(),
                c_entries.len()
            );
            for (s, c) in s_entries.iter().zip(c_entries) {
                assert_eq!(
                    s.get("matcher"),
                    c.get("matcher"),
                    "`{ev}` matcher differs — Codex normalizes tool names to \
                     Claude's vocabulary, so matchers must be identical"
                );
                let sn: Vec<&str> = s["hooks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|h| {
                        h["command"]
                            .as_str()
                            .unwrap()
                            .rsplit('/')
                            .next()
                            .unwrap()
                            .split(|c: char| c == '"' || c.is_whitespace())
                            .next()
                            .unwrap()
                    })
                    .collect();
                let cn: Vec<&str> = c["hooks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|h| {
                        h["command"]
                            .as_str()
                            .unwrap()
                            .rsplit('/')
                            .next()
                            .unwrap()
                            .split(|c: char| c == '"' || c.is_whitespace())
                            .next()
                            .unwrap()
                    })
                    .collect();
                assert_eq!(sn, cn, "`{ev}` wires different handlers");
            }
        }
    }

    #[test]
    fn the_wiring_carries_no_claude_only_variable() {
        let body = crate::templates::EMBEDDED_TEMPLATES
            .get(
                crate::vendors::profile(crate::vendors::LaunchVendor::Codex)
                    .hooks
                    .template,
            )
            .unwrap();
        assert!(
            !body.contains("CLAUDE_PROJECT_DIR"),
            "Codex has no CLAUDE_PROJECT_DIR; use $(git rev-parse --show-toplevel)"
        );
        assert!(
            !body.contains("/.claude/hooks/"),
            "a codex-only machine must not need a .claude/ directory"
        );
        assert!(body.contains("git rev-parse --show-toplevel"));
    }

    #[test]
    fn git_hooks_are_not_duplicated_into_the_codex_tree() {
        // commit-msg / pre-commit / post-commit are git-level and vendor
        // independent; installing them here would double-fire them.
        let names = handler_names();
        for git_hook in [
            "aida-commit-msg",
            "aida-pre-commit.sh",
            "aida-post-commit.sh",
        ] {
            assert!(
                !names.iter().any(|n| n == git_hook),
                "{git_hook} is a git hook and must not be wired into Codex"
            );
        }
        assert!(!names.is_empty(), "expected some handlers");
    }

    #[test]
    fn scaffolding_writes_wiring_and_handlers() {
        let dir = std::env::temp_dir().join(format!("aida-codexhooks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let out = scaffold_codex_hooks(&dir, false).unwrap();
        assert!(dir.join(HOOKS_JSON_REL).is_file(), "wiring must exist");
        assert!(
            out.written.iter().any(|w| w == HOOKS_JSON_REL),
            "wiring should be reported as written"
        );
        for name in handler_names() {
            assert!(
                dir.join(HOOKS_DIR_REL).join(&name).is_file(),
                "handler {name} must be written — the wiring points at it"
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scaffolding_is_idempotent_and_preserves_edits() {
        let dir = std::env::temp_dir().join(format!("aida-codexhooks-idem-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        scaffold_codex_hooks(&dir, false).unwrap();
        let wiring = dir.join(HOOKS_JSON_REL);
        std::fs::write(&wiring, "{\"hooks\":{}}\n").unwrap();

        let out = scaffold_codex_hooks(&dir, false).unwrap();
        assert_eq!(
            std::fs::read_to_string(&wiring).unwrap(),
            "{\"hooks\":{}}\n",
            "a user's edited wiring must survive"
        );
        assert!(out.skipped_existing.iter().any(|s| s == HOOKS_JSON_REL));

        let forced = scaffold_codex_hooks(&dir, true).unwrap();
        assert!(forced.written.iter().any(|w| w == HOOKS_JSON_REL));
        std::fs::remove_dir_all(&dir).ok();
    }

    // trace:TASK-1744 | ai:antigravity
    #[test]
    fn antigravity_scaffolding_merges_existing_object_without_destroying_third_party_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let wiring = root.join(AGY_HOOKS_JSON_REL);
        std::fs::create_dir_all(wiring.parent().unwrap()).unwrap();
        std::fs::write(
            &wiring,
            r#"{"z-user-hook":{"PreInvocation":[{"type":"command","command":"echo z"}]},"b-user-hook":{"PreInvocation":[{"type":"command","command":"echo b"}]}}"#,
        )
        .unwrap();
        assert!(!antigravity_hooks_wired(root));

        let out = scaffold_antigravity_hooks(root, false).unwrap();
        assert!(out.warning.is_none());
        assert!(out.written.iter().any(|w| w == AGY_HOOKS_JSON_REL));
        assert!(antigravity_hooks_wired(root));

        let merged_raw = std::fs::read_to_string(&wiring).unwrap();
        let z_pos = merged_raw
            .find("\"z-user-hook\"")
            .expect("z-user-hook present");
        let b_pos = merged_raw
            .find("\"b-user-hook\"")
            .expect("b-user-hook present");
        let aida_pos = merged_raw
            .find(&format!("\"{AGY_EXPIRY_NOTICE_KEY}\""))
            .expect("aida key present");
        assert!(
            z_pos < b_pos && b_pos < aida_pos,
            "non-alphabetical third-party key order must be preserved, got:\n{merged_raw}"
        );

        // Customize AIDA's own key while keeping z-user-hook before b-user-hook;
        // non-force pass must leave it untouched.
        let customized_text = r#"{
  "z-user-hook": {
    "PreInvocation": [
      {
        "type": "command",
        "command": "echo z"
      }
    ]
  },
  "aida-grant-expiry-notice": {
    "PreInvocation": [
      {
        "type": "command",
        "command": "custom-aida"
      }
    ]
  },
  "b-user-hook": {
    "PreInvocation": [
      {
        "type": "command",
        "command": "echo b"
      }
    ]
  },
  "aida-mail-notice": {
    "SessionStart": [
      {
        "type": "command",
        "command": "custom-mail"
      }
    ]
  },
  "aida-mail-notice-turn": {
    "PreInvocation": [
      {
        "type": "command",
        "command": "custom-mail-turn"
      }
    ]
  }
}
"#;
        std::fs::write(&wiring, customized_text).unwrap();

        let second = scaffold_antigravity_hooks(root, false).unwrap();
        assert!(second
            .skipped_existing
            .iter()
            .any(|s| s == AGY_HOOKS_JSON_REL));
        assert_eq!(std::fs::read_to_string(&wiring).unwrap(), customized_text);

        // Forced pass overwrites only AIDA's key in-place while preserving z-user-hook before b-user-hook.
        let forced = scaffold_antigravity_hooks(root, true).unwrap();
        assert!(forced.written.iter().any(|w| w == AGY_HOOKS_JSON_REL));
        let forced_raw = std::fs::read_to_string(&wiring).unwrap();
        let z_after = forced_raw.find("\"z-user-hook\"").unwrap();
        let aida_after = forced_raw
            .find(&format!("\"{AGY_EXPIRY_NOTICE_KEY}\""))
            .unwrap();
        let b_after = forced_raw.find("\"b-user-hook\"").unwrap();
        assert!(
            z_after < aida_after && aida_after < b_after,
            "in-place forced update must preserve key order, got:\n{forced_raw}"
        );
        let after_force: serde_json::Value = serde_json::from_str(&forced_raw).unwrap();
        assert!(after_force[AGY_EXPIRY_NOTICE_KEY]
            .to_string()
            .contains("aida-grant-expiry-notice.sh"));
    }

    /// An install from before the agy SessionStart wiring gains it on a
    /// non-force pass, leaving the customized expiry key alone.
    // trace:TASK-2253.acd91dca | ai:claude
    #[test]
    fn antigravity_scaffolding_adds_missing_mail_notice_key_without_force() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let wiring = root.join(AGY_HOOKS_JSON_REL);
        std::fs::create_dir_all(wiring.parent().unwrap()).unwrap();
        std::fs::write(
            &wiring,
            r#"{"aida-grant-expiry-notice":{"PreInvocation":[{"type":"command","command":"custom-aida"}]}}"#,
        )
        .unwrap();

        let out = scaffold_antigravity_hooks(root, false).unwrap();
        assert!(out.written.iter().any(|w| w == AGY_HOOKS_JSON_REL));
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&wiring).unwrap()).unwrap();
        assert_eq!(
            v[AGY_EXPIRY_NOTICE_KEY]["PreInvocation"][0]["command"],
            "custom-aida"
        );
        assert!(v["aida-mail-notice"]["SessionStart"][0]["command"]
            .as_str()
            .unwrap()
            .contains("aida-mail-notice.sh"));
        // An existing install also gains the independently managed turn
        // handler while retaining its customized expiry wiring.
        // trace:STORY-1843.ac0eef56 | ai:codex
        assert!(v["aida-mail-notice-turn"]["PreInvocation"][0]["command"]
            .as_str()
            .unwrap()
            .contains("--event PreInvocation"));
        assert_eq!(
            v["aida-mail-notice-turn"]["PreInvocation"][0]["timeout"],
            10
        );
        assert!(root
            .join(AGY_HOOKS_DIR_REL)
            .join("aida-mail-notice.sh")
            .is_file());
    }

    /// Antigravity names `SessionStart` as a flat handler list whose stdin is
    /// camelCase (`conversationId`, no event name) and whose stdout must be a
    /// JSON object. Run the scaffolded agy SessionStart command, as agy does
    /// (`sh -c`, cwd = the dir holding hooks.json), against a recording fake
    /// `aida`: the payload and the named event reach `aida awaiting --notice`
    /// — the session-start path that runs the host-side dead-row gc — and the
    /// notice comes back as an injected system message. Contract source:
    /// Antigravity CLI 1.4.2 builtin `agy-customizations/docs/hooks.md`,
    /// "### 3. `SessionStart` Contract".
    // trace:TASK-2253.acd91dca | ai:claude
    #[cfg(unix)]
    #[test]
    fn agy_session_start_hook_reaches_the_notice_session_start_path() {
        use std::io::Write;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let st = crate::test_git::git_command()
            .args(["init", "-q"])
            .current_dir(root)
            .status()
            .unwrap();
        assert!(st.success());
        scaffold_antigravity_hooks(root, false).unwrap();

        let wiring: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(root.join(AGY_HOOKS_JSON_REL)).unwrap())
                .unwrap();
        let command = wiring["aida-mail-notice"]["SessionStart"][0]["command"]
            .as_str()
            .expect("agy SessionStart handler is flat")
            .to_string();

        let bin = root.join("fake-bin");
        std::fs::create_dir_all(&bin).unwrap();
        let record = root.join("notice-env");
        let fake = bin.join("aida");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\n[ \"$1 $2\" = \"awaiting --notice\" ] || exit 0\n\
                 {{ printf 'PAYLOAD=%s\\n' \"$AIDA_HOOK_PAYLOAD\"; printf 'EVENT=%s\\n' \"$AIDA_HOOK_EVENT\"; }} > '{}'\n\
                 printf 'Current date/time: now \"q\" a\\\\b\\nAwaiting you: 1 mail\\n'\n",
                record.display()
            ),
        )
        .unwrap();
        set_executable(&fake);

        let payload =
            r#"{"conversationId":"agy-conv-1","workspacePaths":["/w"],"modelName":"auto"}"#;
        let path = format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg(&command)
            .current_dir(root.join(".agents"))
            .env("PATH", path)
            .env_remove("AIDA_SESSION_PROJECT")
            .env_remove("CLAUDE_PROJECT_DIR")
            .env_remove("CODEX_PROJECT_DIR")
            .env_remove("AIDA_HOOK_EVENT")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());

        let seen = std::fs::read_to_string(&record).expect("aida awaiting --notice ran");
        assert!(seen.contains(&format!("PAYLOAD={payload}")), "{seen}");
        assert!(seen.contains("EVENT=SessionStart"), "{seen}");

        let out: serde_json::Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|e| panic!("agy stdout must be JSON ({e}): {:?}", output.stdout));
        assert_eq!(
            out["injectSteps"][0]["systemMessage"]["systemMessage"],
            "Current date/time: now \"q\" a\\b\nAwaiting you: 1 mail"
        );
    }

    // trace:TASK-1744 | ai:antigravity
    #[test]
    fn antigravity_scaffolding_leaves_invalid_or_non_object_hooks_json_untouched_even_with_force() {
        for raw in ["not-json{{{", "[1, 2, 3]\n", "\"string\"\n"] {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path();
            let wiring = root.join(AGY_HOOKS_JSON_REL);
            std::fs::create_dir_all(wiring.parent().unwrap()).unwrap();
            std::fs::write(&wiring, raw).unwrap();

            for force in [false, true] {
                let out = scaffold_antigravity_hooks(root, force).unwrap();
                assert!(
                    out.warning.is_some(),
                    "expected warning for {raw:?} (force={force})"
                );
                assert!(out.written.is_empty());
                assert_eq!(
                    std::fs::read_to_string(&wiring).unwrap(),
                    raw,
                    "invalid/non-object hooks.json must never be overwritten (force={force})"
                );
                assert!(
                    !root
                        .join(AGY_HOOKS_DIR_REL)
                        .join("aida-grant-expiry-notice.sh")
                        .exists(),
                    "handler script must not be dropped when hooks.json is invalid (force={force})"
                );
            }
        }
    }
}

/// BUG-2395: the shared hook scripts must work unchanged under Codex. These
/// run the embedded masters against fixture repos and a fake HOME only.
// trace:BUG-2395 | ai:claude
#[cfg(all(test, unix))]
mod bug_2395_vendor_neutral_hooks {
    use std::path::Path;
    use std::process::{Command, Stdio};

    fn write_hook(dir: &Path, name: &str) -> std::path::PathBuf {
        let body = crate::templates::EMBEDDED_TEMPLATES
            .get(format!("hooks/{name}").as_str())
            .copied()
            .expect("embedded hook");
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    fn git(repo: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    /// Runs `aida-commit-msg` with `staged` (path, body) files staged and
    /// returns its combined output.
    fn commit_msg_output(staged: &[(&str, &str)]) -> String {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        for (rel, body) in staged {
            let p = repo.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, body).unwrap();
            git(&repo, &["add", rel]);
        }
        let hook = write_hook(tmp.path(), "aida-commit-msg");
        let msg = tmp.path().join("MSG");
        std::fs::write(&msg, "chore(scaffold): refresh hooks\n").unwrap();
        let out = Command::new("bash")
            .arg(&hook)
            .arg(&msg)
            .current_dir(&repo)
            .env("HOME", tmp.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env_remove("AIDA_RELEASE")
            .env_remove("AIDA_COMMIT_STRICT")
            .output()
            .unwrap();
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    }

    #[test]
    fn commit_msg_ignores_foreign_traces_in_codex_and_agents_mirrors() {
        let traced = "#!/bin/bash\n# trace:TASK-9999 | ai:claude\n";
        let mirrors = commit_msg_output(&[
            (".codex/hooks/aida-x.sh", traced),
            (".agents/hooks/aida-y.sh", traced),
        ]);
        assert!(
            !mirrors.contains("TASK-9999") && !mirrors.contains("AI-traced"),
            "scaffold mirrors must not trigger trace warnings: {mirrors}"
        );
        let source = commit_msg_output(&[("src/x.sh", traced)]);
        assert!(
            source.contains("AI-traced"),
            "control: a source file still warns: {source}"
        );
    }

    /// Runs `aida-capture-prompt.sh` with `payload` on stdin and returns stdout.
    fn capture_prompt(home: &Path, payload: &str, envs: &[(&str, &str)]) -> String {
        let hook = write_hook(home, "aida-capture-prompt.sh");
        let mut cmd = Command::new("bash");
        cmd.arg(&hook)
            .current_dir(home)
            .env("HOME", home)
            .env_remove("CODEX_PROJECT_DIR")
            .env_remove("CLAUDE_PROJECT_DIR")
            .env_remove("AIDA_SESSION_PROJECT")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().unwrap();
        use std::io::Write;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        String::from_utf8_lossy(&child.wait_with_output().unwrap().stdout).into_owned()
    }

    #[test]
    fn capture_prompt_counts_codex_skill_and_speaks_both_vendors() {
        if Command::new("jq").arg("--version").output().is_err() {
            eprintln!("skipping: jq not installed");
            return;
        }
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path();
        let transcript = home.join("t.jsonl");
        let payload = format!(
            r#"{{"session_id":"s1","transcript_path":"{}"}}"#,
            transcript.display()
        );

        std::fs::write(&transcript, "{\"text\":\"this is worth noting\"}\n").unwrap();
        let fired = capture_prompt(home, &payload, &[]);
        assert!(fired.contains("/aida-capture"), "{fired}");
        assert!(
            fired.contains("$aida-capture"),
            "Codex wording missing: {fired}"
        );

        std::fs::write(
            &transcript,
            "{\"text\":\"this is worth noting\"}\n{\"text\":\"ran $aida-capture\"}\n",
        )
        .unwrap();
        assert!(
            capture_prompt(home, &payload, &[]).is_empty(),
            "$aida-capture counts as filing activity"
        );

        // A Codex payload without transcript_path never reads ~/.claude.
        let project = home.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let encoded = project.display().to_string().replace('/', "-");
        let claude_dir = home.join(".claude/projects").join(encoded);
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(claude_dir.join("s2.jsonl"), "{\"text\":\"worth noting\"}\n").unwrap();
        let project_str = project.display().to_string();
        let no_path = r#"{"session_id":"s2"}"#;
        assert!(
            !capture_prompt(home, no_path, &[("CLAUDE_PROJECT_DIR", &project_str)]).is_empty(),
            "control: Claude falls back to its transcript layout"
        );
        assert!(
            capture_prompt(home, no_path, &[("CODEX_PROJECT_DIR", &project_str)]).is_empty(),
            "Codex must not fall back to the Claude transcript layout"
        );
    }
}
