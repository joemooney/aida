//! Project-owned validation commands, selected only by trusted default-branch policy.
// trace:BUG-2512 | ai:codex
use anyhow::{Context, Result};
use std::path::Path;

#[derive(Default, serde::Deserialize)]
pub(crate) struct ProjectChecks {
    pub(crate) format: Option<String>,
    pub(crate) test: Option<String>,
    pub(crate) fast_check: Option<String>,
}

pub(crate) fn load(root: &Path) -> Result<ProjectChecks> {
    parse(crate::trusted_config::read_trusted_config_toml(root).as_deref())
}

fn parse(body: Option<&str>) -> Result<ProjectChecks> {
    #[derive(Default, serde::Deserialize)]
    struct Config {
        #[serde(default)]
        validation: ProjectChecks,
    }
    let mut checks = match body {
        Some(body) => {
            toml::from_str::<Config>(body)
                .context("parsing trusted project validation commands")?
                .validation
        }
        None => ProjectChecks::default(),
    };
    for command in [&mut checks.format, &mut checks.test, &mut checks.fast_check] {
        *command = command.take().filter(|value| !value.trim().is_empty());
    }
    Ok(checks)
}

impl ProjectChecks {
    pub(crate) fn prompt(&self) -> String {
        let mut out = String::from("Project validation commands (trusted default-branch config):");
        for (name, command) in [("format", &self.format), ("test", &self.test)] {
            match command {
                Some(command) => out.push_str(&format!("\n{name}: `{command}`")),
                None => out.push_str(&format!(
                    "\n{name}: not configured; select relevant project checks."
                )),
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bug_2512_python_project_commands_and_absent_configuration() {
        let checks = parse(Some(
            r#"
[validation]
format = "ruff format ."
test = "python3 -m unittest discover"
fast_check = "python3 -m unittest discover"
"#,
        ))
        .unwrap();
        let prompt = checks.prompt();
        assert!(prompt.contains("ruff format ."));
        assert!(prompt.contains("python3 -m unittest discover"));
        assert!(!prompt.contains("cargo"));
        assert!(!prompt.contains("make"));
        assert_eq!(
            checks.fast_check.as_deref(),
            Some("python3 -m unittest discover")
        );
        for body in [
            None,
            Some("[agents]\nvendor = 'codex'"),
            Some("[validation]\nfast_check = ' '"),
        ] {
            assert!(parse(body).unwrap().fast_check.is_none());
        }
        assert!(parse(Some("[validation]\nfast_check = 42")).is_err());
    }
    #[test]
    fn bug_2512_trusted_python_commands_ignore_lane_config() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let output = crate::test_git::git_command()
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "fixture@example.invalid"]);
        git(&["config", "user.name", "Fixture"]);
        std::fs::create_dir(dir.path().join(".aida")).unwrap();
        let config = dir.path().join(".aida/config.toml");
        std::fs::write(&config, "[validation]\nformat = 'ruff format .'\ntest = 'python3 -m unittest discover'\nfast_check = 'python3 -m unittest discover'\n").unwrap();
        git(&["add", "."]);
        git(&[
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "-qm",
            "trusted python commands",
        ]);
        git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git(&[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ]);
        git(&["checkout", "-qb", "lane"]);
        std::fs::write(
            &config,
            "[validation]\nformat = 'untrusted-format'\nfast_check = 'untrusted-gate'\n",
        )
        .unwrap();
        let checks = load(dir.path()).unwrap();
        assert_eq!(
            checks.fast_check.as_deref(),
            Some("python3 -m unittest discover")
        );
        let prompt = crate::agent_launch_prompt::role_launch_prompt(
            dir.path(),
            "codex",
            Some("implementer"),
            Some("BUG-2512"),
            "open a PR",
        );
        assert!(prompt.contains("ruff format ."));
        assert!(prompt.contains("python3 -m unittest discover"));
        assert!(!prompt.contains("cargo fmt"));
        assert!(!prompt.contains("untrusted-format"));
    }
}
