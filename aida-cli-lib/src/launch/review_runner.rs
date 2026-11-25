// trace:STORY-3120 | ai:claude
//! The one hardened spawn path for `aida review prepare --run-review` runners.
//! Every runner (claude, codex, `AIDA_REVIEW_RUNNER_CMD`) starts in a fresh empty
//! directory with a cleared environment, reads the prompt on stdin, and is killed
//! with its process group at the deadline or when its output exceeds the caps.
// Wired into `--run-review` by the next slice; only the tests call it yet.
#![cfg_attr(not(test), allow(dead_code))]

use crate::vendors::review_runner::ReviewVendor;
use anyhow::{anyhow, bail, ensure, Context, Result};
use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(crate) const STDOUT_CAP: usize = 4 << 20;
pub(crate) const STDERR_CAP: usize = 64 << 10;
const RUNNER_CMD: &str = "AIDA_REVIEW_RUNNER_CMD";
const BASE_ENV: [&str; 4] = ["PATH", "HOME", "LANG", "TERM"];
/// One runner invocation. `parent_env` is the operator's environment: the only
/// source of `AIDA_REVIEW_RUNNER_CMD` and of every variable the runner sees.
pub(crate) struct RunnerCall<'a> {
    pub agent: Option<&'static ReviewVendor>,
    pub checkout: &'a Path,
    pub parent_env: &'a [(OsString, OsString)],
    pub pass: &'a str,
    pub deadline: Duration,
    pub stdout_cap: usize,
    pub stderr_cap: usize,
}

impl RunnerCall<'_> {
    fn var(&self, key: &str) -> Option<&OsStr> {
        let found = self
            .parent_env
            .iter()
            .find(|(k, v)| k == key && !v.is_empty());
        found.map(|(_, v)| v.as_os_str())
    }

    /// Absolute runner path; refused when relative, missing, not a regular
    /// file, or inside the checkout under review.
    fn program(&self) -> Result<PathBuf> {
        let path = match (self.var(RUNNER_CMD), self.agent) {
            (Some(cmd), _) => PathBuf::from(cmd),
            (None, Some(agent)) => std::env::split_paths(self.var("PATH").unwrap_or_default())
                .map(|dir| dir.join(agent.program))
                .find(|p| p.is_absolute() && p.is_file())
                .with_context(|| format!("{} not found on PATH", agent.program))?,
            (None, None) => bail!("--run-review needs --agent <claude|codex> or {RUNNER_CMD}"),
        };
        ensure!(path.is_absolute(), "{RUNNER_CMD} must be an absolute path");
        let real = path.canonicalize().context("review runner not found")?;
        ensure!(real.is_file(), "review runner must be a regular file");
        let inside = real.starts_with(self.checkout.canonicalize()?);
        ensure!(!inside, "review runner must live outside the checkout");
        Ok(real)
    }

    /// Run the runner on `prompt` and return its stdout. A spawn failure,
    /// overrun or non-zero exit is an error. An unread prompt is detected only
    /// when it exceeds the pipe buffer (about 64 KiB); a runner that exits before
    /// reading a smaller prompt fails only on its own exit status or output.
    pub(crate) fn run(&self, prompt: &str) -> Result<String> {
        let program = self.program()?;
        let vendor = self.agent.filter(|_| self.var(RUNNER_CMD).is_none());
        let workdir = tempfile::tempdir()?;
        let mut cmd = Command::new(&program);
        cmd.env_clear().current_dir(workdir.path()).process_group(0);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let auth = vendor.map_or(&[][..], |agent| agent.auth);
        for (key, value) in self.parent_env {
            let k = &*key.to_string_lossy();
            if BASE_ENV.contains(&k) || k.starts_with("LC_") || auth.contains(&k) {
                cmd.env(key, value);
            }
        }
        cmd.env("AIDA_REVIEW_PASS", self.pass);
        if let Some(agent) = self.agent {
            cmd.env("AIDA_REVIEW_RESOLVED_AGENT", agent.program);
        }
        cmd.args(vendor.map_or(&[][..], |agent| agent.argv));
        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawn {}", program.display()))?;
        let group = child.id() as libc::pid_t;
        let tripped = Arc::new(AtomicBool::new(false));
        let capped = |stream: Option<Box<dyn Read + Send>>, cap: usize| {
            let (tripped, stream) = (Arc::clone(&tripped), stream.context("runner pipe")?);
            Ok::<_, anyhow::Error>(std::thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = stream.take(cap as u64 + 1).read_to_end(&mut buf);
                tripped.fetch_or(buf.len() > cap, SeqCst);
                buf
            }))
        };
        let out = capped(
            child.stdout.take().map(|s| Box::new(s) as _),
            self.stdout_cap,
        )?;
        let err = capped(
            child.stderr.take().map(|s| Box::new(s) as _),
            self.stderr_cap,
        )?;
        let (mut stdin, body) = (child.stdin.take().context("stdin")?, prompt.to_owned());
        let writer = std::thread::spawn(move || {
            // The aida binary restores SIGPIPE to SIG_DFL (BUG-99). Block it on this
            // thread so a runner that closes stdin early yields EPIPE (a run error),
            // not a killed aida. A pending SIGPIPE is discarded when the thread exits.
            // SAFETY: a local sigset; pthread_sigmask changes only this thread's mask.
            unsafe {
                let mut set: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut set);
                libc::sigaddset(&mut set, libc::SIGPIPE);
                libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
            }
            stdin.write_all(body.as_bytes())
        });
        let deadline = Instant::now() + self.deadline;
        let finished = loop {
            // Peek without reaping, so the group id cannot be reused before the kill.
            // SAFETY: waitid writes only into `info`; WNOWAIT leaves the child unreaped.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            let flags = libc::WEXITED | libc::WNOHANG | libc::WNOWAIT;
            let rc = unsafe { libc::waitid(libc::P_PID, group as libc::id_t, &mut info, flags) };
            // On a waitid error, stop polling: the group is killed and reaped below,
            // so a killed runner fails on its exit status, never leaks or passes.
            // SAFETY: waitid filled `info` for a child state change (or left it zeroed).
            if rc != 0 || unsafe { info.si_pid() } != 0 {
                break true;
            }
            if tripped.load(SeqCst) || Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        // Kill the whole group: stragglers would hold the pipes open.
        // SAFETY: kill(2) on the runner's own, still unreaped, process group.
        unsafe { libc::kill(-group, libc::SIGKILL) };
        let status = child.wait()?;
        let wrote = writer
            .join()
            .map_err(|_| anyhow!("stdin writer panicked"))?;
        let stdout = out.join().map_err(|_| anyhow!("stdout reader panicked"))?;
        let stderr = err.join().map_err(|_| anyhow!("stderr reader panicked"))?;
        ensure!(
            !tripped.load(SeqCst),
            "review runner output exceeded its cap"
        );
        let secs = self.deadline.as_secs();
        ensure!(finished, "review runner exceeded its {secs}s deadline");
        let detail = String::from_utf8_lossy(&stderr);
        ensure!(
            status.success(),
            "review runner failed ({status}): {}",
            detail.trim()
        );
        wrote.context("review runner did not read the whole prompt")?;
        String::from_utf8(stdout).context("review runner output is not UTF-8")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vendors::review_runner::review_vendor;

    // Fake vendor binary: runs a planted hook in its cwd if one exists, then
    // dumps its cwd listing, env, argv and the byte count read on stdin.
    const DUMP: &str = "#!/bin/sh\n[ -f .claude/hook.sh ] && sh .claude/hook.sh\n\
        echo \"cwd=$(ls -A | tr '\\n' ,)\"; env | sed 's/^/env=/'\n\
        for a in \"$@\"; do echo \"arg=$a\"; done; echo \"stdin=$(wc -c | tr -d ' ')\"\n";

    fn call<'a>(dir: &'a Path, env: &'a [(OsString, OsString)]) -> RunnerCall<'a> {
        let deadline = Duration::from_secs(20);
        let (stdout_cap, stderr_cap) = (STDOUT_CAP, STDERR_CAP);
        let (agent, pass) = (None, "finder");
        RunnerCall {
            agent,
            checkout: dir,
            parent_env: env,
            pass,
            deadline,
            stdout_cap,
            stderr_cap,
        }
    }

    // Copy via install(1): a parallel test's fork can inherit a write fd we hold
    // on the executable itself, and exec would then fail with ETXTBSY.
    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let (src, path) = (dir.join(format!("{name}.src")), dir.join(name));
        std::fs::write(&src, body).unwrap();
        let ok = Command::new("install")
            .arg("-m755")
            .args([&src, &path])
            .status();
        assert!(ok.unwrap().success(), "install {}", path.display());
        path
    }

    fn vendor(name: &str) -> Option<&'static ReviewVendor> {
        Some(review_vendor(name).unwrap())
    }

    fn vars(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs.iter().map(|(k, v)| (k.into(), v.into())).collect()
    }

    #[test]
    fn s3a_runner_spawn_contract() {
        let (bin, checkout) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (bin, dir) = (bin.path(), checkout.path());
        let marker = dir.join("hook-ran");
        std::fs::create_dir(dir.join(".claude")).unwrap();
        std::fs::write(dir.join(".claude/settings.json"), "{}").unwrap();
        std::fs::write(
            dir.join(".claude/hook.sh"),
            format!("touch {}\n", marker.display()),
        )
        .unwrap();
        let custom = script(bin, "custom", DUMP).display().to_string();
        script(bin, "claude", DUMP);
        script(bin, "codex", DUMP);
        let path = format!("{}:/usr/bin:/bin", bin.display());
        let mut env = vars(&[
            ("PATH", &path),
            ("HOME", "/nonexistent"),
            ("LC_ALL", "C"),
            ("GH_TOKEN", "sentinel-gh"),
            ("AIDA_SESSION_GRANT", "sentinel-grant"),
            ("ANTHROPIC_API_KEY", "sentinel-anthropic"),
            ("OPENAI_API_KEY", "sentinel-openai"),
        ]);
        let prompt = "x".repeat(200 << 10); // over Linux's 128 KiB per-argument limit
        let stdin = format!("stdin={}\n", prompt.len());
        let mut cases = vec![
            (vendor("claude"), "anthropic", "openai"),
            (vendor("codex"), "openai", "anthropic"),
        ];
        for (agent, own, other) in cases.drain(..) {
            let out = RunnerCall {
                agent,
                ..call(dir, &env)
            }
            .run(&prompt)
            .unwrap();
            assert!(out.contains("cwd=\n") && out.contains(&stdin), "{out}");
            assert!(out.contains(own) && !out.contains(other), "{out}");
            assert!(out.contains("env=LC_ALL=C") && out.contains("env=AIDA_REVIEW_PASS=finder"));
            assert!(
                !out.contains("sentinel-gh") && !out.contains("sentinel-grant"),
                "{out}"
            );
            let argv: Vec<_> = out.lines().filter_map(|l| l.strip_prefix("arg=")).collect();
            assert_eq!(argv, agent.unwrap().argv, "prompt or extra args in argv");
        }
        env.push((RUNNER_CMD.into(), custom.into()));
        let agent = vendor("claude");
        let out = RunnerCall {
            agent,
            ..call(dir, &env)
        }
        .run(&prompt)
        .unwrap();
        assert!(out.contains("cwd=\n") && out.contains(&stdin), "{out}");
        assert!(!out.contains("sentinel-") && !out.contains("arg="), "{out}");
        assert!(
            out.contains("env=AIDA_REVIEW_RESOLVED_AGENT=claude"),
            "{out}"
        );
        assert!(!marker.exists(), "planted checkout hook ran");

        let refuse = |value: &str| {
            let mut env = env.clone();
            env.last_mut().unwrap().1 = value.into();
            call(dir, &env).run("p").unwrap_err().to_string()
        };
        assert!(refuse("scripts/review").contains("absolute"));
        let inside = script(dir, "runner", DUMP);
        assert!(refuse(inside.to_str().unwrap()).contains("outside the checkout"));
        assert!(refuse(bin.to_str().unwrap()).contains("regular file"));
        assert!(
            refuse("").contains("--agent"),
            "empty command must not default"
        );
        for unknown in ["gemini", "agy", "antigravity", "", "claude-x"] {
            assert!(review_vendor(unknown).is_err(), "{unknown} accepted");
        }
        assert_eq!(review_vendor(" Codex ").unwrap(), vendor("codex").unwrap());
    }

    #[test]
    fn s3a_runner_deadline_and_output_caps_fail_closed() {
        let (bin, checkout) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let run = |body: &str, prompt: &str, secs: u64, caps: (usize, usize)| {
            let cmd = script(bin.path(), "runner", body).display().to_string();
            let env = vars(&[("PATH", "/usr/bin:/bin"), (RUNNER_CMD, &cmd)]);
            let deadline = Duration::from_secs(secs);
            let (stdout_cap, stderr_cap) = caps;
            let runner = RunnerCall {
                deadline,
                stdout_cap,
                stderr_cap,
                ..call(checkout.path(), &env)
            };
            runner.run(prompt).unwrap_err().to_string()
        };
        let full = (STDOUT_CAP, STDERR_CAP);
        let started = Instant::now();
        assert!(run("#!/bin/sh\nsleep 30\n", "p", 1, full).contains("1s deadline"));
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "process group not killed"
        );
        let noisy = "#!/bin/sh\nhead -c 50000 /dev/zero >&2\nhead -c 50000 /dev/zero\nsleep 30\n";
        for caps in [(STDOUT_CAP, 1024), (1024, STDERR_CAP)] {
            assert!(run(noisy, "p", 20, caps).contains("exceeded its cap"));
        }
        assert!(run("#!/bin/sh\necho boom >&2\nexit 3\n", "p", 20, full).contains("boom"));
        let deaf = run("#!/bin/sh\necho '{}'\n", &"x".repeat(1 << 20), 20, full);
        assert!(deaf.contains("whole prompt"), "{deaf}");
    }

    /// Re-runs itself in a child test process with SIGPIPE at SIG_DFL, as the
    /// aida binary has it; libtest ignores SIGPIPE and would hide a kill.
    #[test]
    fn s3a_runner_survives_default_sigpipe() {
        // A second, never-matching filter marks the child run.
        const CHILD: &str = "s3a-sigpipe-child";
        if !std::env::args().any(|arg| arg == CHILD) {
            let name = format!("{}::s3a_runner_survives_default_sigpipe", module_path!());
            let name = name.split_once("::").unwrap().1;
            let out = Command::new(std::env::args_os().next().unwrap())
                .args(["--exact", name, CHILD, "--nocapture", "--test-threads=1"])
                .output()
                .unwrap();
            let text = String::from_utf8_lossy(&out.stdout);
            assert!(out.status.success() && text.contains("1 passed"), "{out:?}");
            return;
        }
        // SAFETY: sets the aida binary's own disposition, in this child process only.
        unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
        let (bin, checkout) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let prompt = "x".repeat(200 << 10);
        let noisy = "#!/bin/sh\nhead -c 50000 /dev/zero\nsleep 30\n";
        let cases = [
            ("#!/bin/sh\nexit 0\n", 20, 1024, "whole prompt"),
            ("#!/bin/sh\nsleep 30\n", 1, STDOUT_CAP, "1s deadline"),
            (noisy, 20, 1024, "exceeded its cap"),
        ];
        for (body, secs, stdout_cap, want) in cases {
            let cmd = script(bin.path(), "runner", body).display().to_string();
            let env = vars(&[("PATH", "/usr/bin:/bin"), (RUNNER_CMD, &cmd)]);
            let runner = RunnerCall {
                deadline: Duration::from_secs(secs),
                stdout_cap,
                ..call(checkout.path(), &env)
            };
            let err = runner.run(&prompt).unwrap_err().to_string();
            assert!(err.contains(want), "{want}: {err}");
        }
    }
}
