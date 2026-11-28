// trace:STORY-3120 | ai:claude
//! The one hardened spawn path for `aida review prepare --run-review` runners.
//! Every runner (claude, codex, agy, `AIDA_REVIEW_RUNNER_CMD`) starts in a fresh empty
//! directory with a cleared environment, reads the prompt on stdin, and is killed
//! with its process group at the deadline or when its output exceeds the caps.

use super::review_tuning::{ReviewTuning, BASE_EFFORTS};
use crate::vendors::review_runner::{AuthRequired, ReviewVendor};
use anyhow::{anyhow, bail, ensure, Context, Result};
use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
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
/// The network route only: a proxied or sandboxed seat reaches its vendor API
/// solely through these, and without them claude retries until the deadline.
/// Matched in either case, as curl and the vendor CLIs read both.
// trace:BUG-2775 | ai:claude
const PROXY_ENV: [&str; 4] = ["HTTPS_PROXY", "HTTP_PROXY", "NO_PROXY", "ALL_PROXY"];
/// How much of a failed runner's stderr its error quotes.
const STDERR_TAIL: usize = 600;
/// How long the pipe threads may outlive the killed group before the run fails.
const JOIN_GRACE: Duration = Duration::from_secs(5);
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
    /// Model and effort, already validated by `review_tuning::resolve`.
    pub tuning: ReviewTuning,
}

impl RunnerCall<'_> {
    pub(crate) fn var(&self, key: &str) -> Option<&OsStr> {
        let found = self
            .parent_env
            .iter()
            .find(|(k, v)| k == key && !v.is_empty());
        found.map(|(_, v)| v.as_os_str())
    }

    /// Whether `AIDA_REVIEW_RUNNER_CMD` replaces the vendor.
    // trace:BUG-2775 | ai:claude
    pub(crate) fn runner_cmd_set(&self) -> bool {
        self.var(RUNNER_CMD).is_some()
    }

    /// Who runs this pass, for error messages: the vendor or the custom command.
    pub(crate) fn label(&self) -> &'static str {
        self.vendor().map_or(RUNNER_CMD, |agent| agent.program)
    }

    /// The vendor whose argv and auth apply: none for `AIDA_REVIEW_RUNNER_CMD`.
    pub(crate) fn vendor(&self) -> Option<&'static ReviewVendor> {
        self.agent.filter(|_| self.var(RUNNER_CMD).is_none())
    }

    /// Whether this pass is verified unable to read local files: a custom
    /// runner never is.
    // trace:BUG-2783 | ai:claude
    pub(crate) fn no_read(&self) -> bool {
        self.vendor().is_some_and(|agent| agent.no_read)
    }

    /// `agent`'s binary on the operator's PATH, as `run` would resolve it.
    pub(crate) fn on_path(&self, agent: &ReviewVendor) -> Option<PathBuf> {
        std::env::split_paths(self.var("PATH").unwrap_or_default())
            .map(|dir| dir.join(agent.program))
            .find(|p| p.is_absolute() && p.is_file())
    }

    /// Absolute runner path; refused when relative, missing, not a regular
    /// file, or inside the checkout under review.
    fn program(&self) -> Result<PathBuf> {
        let path = match (self.var(RUNNER_CMD), self.agent) {
            (Some(cmd), _) => PathBuf::from(cmd),
            (None, Some(agent)) => self
                .on_path(agent)
                .with_context(|| format!("{} not found on PATH", agent.program))?,
            (None, None) => bail!("--run-review needs --agent <claude|codex|agy> or {RUNNER_CMD}"),
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
        let vendor = self.vendor();
        let workdir = tempfile::tempdir()?;
        let mut cmd = Command::new(&program);
        cmd.env_clear().current_dir(workdir.path());
        // A new session, so its own process group with no controlling terminal:
        // a vendor CLI that prompts on /dev/tty fails at once instead of
        // stopping on SIGTTIN until the deadline. trace:BUG-2775 | ai:claude
        // SAFETY: setsid(2) is async-signal-safe and touches no parent state.
        unsafe {
            cmd.pre_exec(|| match libc::setsid() {
                -1 => Err(std::io::Error::last_os_error()),
                _ => Ok(()),
            });
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let auth = vendor.map_or(&[][..], |agent| agent.auth);
        for (key, value) in self.parent_env {
            let k = &*key.to_string_lossy();
            let route = PROXY_ENV.iter().any(|p| p.eq_ignore_ascii_case(k));
            if BASE_ENV.contains(&k) || route || k.starts_with("LC_") || auth.contains(&k) {
                cmd.env(key, value);
            }
        }
        cmd.env("AIDA_REVIEW_PASS", self.pass);
        if let Some(agent) = self.agent {
            cmd.env("AIDA_REVIEW_RESOLVED_AGENT", agent.program);
        }
        let (model, effort) = (self.tuning.model(), self.tuning.effort());
        // Re-check against the vendor that runs; a custom runner gets the base set.
        if let Some(level) = effort {
            let allowed = vendor.map_or(BASE_EFFORTS, |agent| agent.efforts);
            let name = vendor.map_or(RUNNER_CMD, |agent| agent.program);
            ensure!(allowed.contains(&level), "{name} has no {level} effort");
        }
        if let Some(model) = model {
            cmd.env("AIDA_REVIEW_RESOLVED_MODEL", model);
        }
        if let Some(level) = effort {
            cmd.env("AIDA_REVIEW_RESOLVED_EFFORT", level);
        }
        cmd.args(vendor.map_or_else(Vec::new, |agent| {
            let mut argv = agent.argv_with(model, effort);
            if let Some(flag) = agent.print_timeout {
                argv.insert(0, format!("{flag}={}s", print_timeout(self.deadline)));
            }
            argv
        }));
        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawn {}", program.display()))?;
        let group = child.id() as libc::pid_t;
        let pipes = pipe_ids([
            child.stdin.as_ref().map(|p| p.as_raw_fd()),
            child.stdout.as_ref().map(|p| p.as_raw_fd()),
            child.stderr.as_ref().map(|p| p.as_raw_fd()),
        ]);
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
        // A descendant that left the group (setsid, double fork) can hold the
        // pipes open forever: wait briefly, then SIGKILL every process found
        // holding one and fail naming them. trace:STORY-3120 | ai:claude
        let open = || !(writer.is_finished() && out.is_finished() && err.is_finished());
        let settle = |limit: Duration| {
            let grace = Instant::now() + limit;
            while open() && Instant::now() < grace {
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        settle(JOIN_GRACE);
        if open() {
            let escaped = pipe_holders(&pipes);
            for pid in &escaped {
                // SAFETY: kill(2) on a process found holding the runner's pipes.
                unsafe { libc::kill(*pid, libc::SIGKILL) };
            }
            settle(JOIN_GRACE);
            let pids: Vec<_> = escaped
                .iter()
                .map(|p| format!("escaped descendant pid {p}"))
                .collect();
            let found = if pids.is_empty() {
                "no escaped descendant found".into()
            } else {
                pids.join(", ") + " (killed)"
            };
            let leaked = if open() {
                "; its pipe threads are leaked"
            } else {
                ""
            };
            bail!("review runner left descendants holding its pipes: {found}{leaked}");
        }
        let wrote = writer
            .join()
            .map_err(|_| anyhow!("stdin writer panicked"))?;
        let stdout = out.join().map_err(|_| anyhow!("stdout reader panicked"))?;
        let stderr = err.join().map_err(|_| anyhow!("stderr reader panicked"))?;
        let (out_cap, err_cap) = (self.stdout_cap, self.stderr_cap);
        ensure!(
            !tripped.load(SeqCst),
            "review runner output exceeded its cap ({out_cap} bytes stdout, {err_cap} stderr)"
        );
        let secs = self.deadline.as_secs();
        ensure!(finished, "review runner exceeded its {secs}s deadline");
        // trace:STORY-3202 | ai:claude
        if let Some(agent) = vendor.filter(|a| !status.success() && a.auth_failed(&stderr)) {
            let (vendor, stderr) = (agent.program, stderr_tail(&stderr));
            return Err(AuthRequired { vendor, stderr }.into());
        }
        ensure!(
            status.success(),
            "review runner failed ({status}); stderr: {}",
            stderr_tail(&stderr)
        );
        wrote.context("review runner did not read the whole prompt")?;
        String::from_utf8(stdout).context("review runner output is not UTF-8")
    }
}

/// The vendor print-mode limit in whole seconds: the runner deadline less a
/// margin for the CLI to exit, never 0 (which means no limit).
// trace:STORY-3202 | ai:claude
fn print_timeout(deadline: Duration) -> u64 {
    let margin = (deadline / 4).min(Duration::from_secs(30));
    deadline.saturating_sub(margin).as_secs().max(1)
}

/// The last `STDERR_TAIL` bytes of `stderr` on one line, so an agent-mode
/// error summary (its first line) keeps the cause.
// trace:BUG-2775 | ai:claude
fn stderr_tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<_> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let joined = lines.join(" / ");
    let cut = joined.len().saturating_sub(STDERR_TAIL);
    let start = (cut..=joined.len()).find(|&i| joined.is_char_boundary(i));
    match &joined[start.unwrap_or(0)..] {
        "" => "(empty)".into(),
        tail if cut > 0 => format!("...{tail}"),
        tail => tail.into(),
    }
}

/// `/proc/self/fd` link targets (`pipe:[inode]`) of the runner's pipe ends.
fn pipe_ids(fds: [Option<std::os::fd::RawFd>; 3]) -> Vec<PathBuf> {
    let link = |fd| std::fs::read_link(format!("/proc/self/fd/{fd}")).ok();
    fds.into_iter().flatten().filter_map(link).collect()
}

/// Other processes with an fd on one of `pipes`: the runner's escaped
/// descendants. Empty where `/proc` is unavailable.
fn pipe_holders(pipes: &[PathBuf]) -> Vec<libc::pid_t> {
    let me = std::process::id() as libc::pid_t;
    let Ok(procs) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let holds = |pid: libc::pid_t| {
        let fds = std::fs::read_dir(format!("/proc/{pid}/fd"))
            .into_iter()
            .flatten();
        fds.flatten()
            .any(|fd| std::fs::read_link(fd.path()).is_ok_and(|t| pipes.contains(&t)))
    };
    let pids = procs
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse().ok());
    let mut found: Vec<_> = pids.filter(|&pid| pid != me && holds(pid)).collect();
    found.sort_unstable();
    found
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
            tuning: ReviewTuning::default(),
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
        script(bin, "agy", DUMP);
        let path = format!("{}:/usr/bin:/bin", bin.display());
        let mut env = vars(&[
            ("PATH", &path),
            ("HOME", "/nonexistent"),
            ("LC_ALL", "C"),
            ("GH_TOKEN", "sentinel-gh"),
            ("AIDA_SESSION_GRANT", "sentinel-grant"),
            ("ANTHROPIC_API_KEY", "sentinel-anthropic"),
            ("OPENAI_API_KEY", "sentinel-openai"),
            ("GEMINI_API_KEY", "sentinel-gemini"),
            ("HTTPS_PROXY", "http://127.0.0.1:3128"),
            ("no_proxy", "localhost"),
        ]);
        let prompt = "x".repeat(200 << 10); // over Linux's 128 KiB per-argument limit
        let stdin = format!("stdin={}\n", prompt.len());
        let mut cases = vec![
            (vendor("claude"), "anthropic", "openai"),
            (vendor("codex"), "openai", "anthropic"),
            (vendor("agy"), "gemini", "anthropic"),
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
            // trace:BUG-2775 | ai:claude
            let proxy = [
                "env=HTTPS_PROXY=http://127.0.0.1:3128",
                "env=no_proxy=localhost",
            ];
            assert!(proxy.iter().all(|p| out.contains(p)), "{out}");
            assert!(
                !out.contains("sentinel-gh") && !out.contains("sentinel-grant"),
                "{out}"
            );
            let mut argv: Vec<_> = out.lines().filter_map(|l| l.strip_prefix("arg=")).collect();
            // agy's own print limit, inside the 20s runner deadline. trace:STORY-3202 | ai:claude
            if agent.unwrap().print_timeout.is_some() {
                assert_eq!(argv.remove(0), "--print-timeout=15s");
            }
            assert_eq!(argv, agent.unwrap().argv, "prompt or extra args in argv");
            // agy: sandboxed plan mode, and session settings that deny every
            // tool rule, reads included. trace:STORY-3202 | ai:claude
            if own == "gemini" {
                assert!(argv.contains(&"--sandbox") && argv.contains(&"--mode=plan"));
                let settings = argv.iter().find_map(|a| a.strip_prefix("--settings="));
                let settings: serde_json::Value =
                    serde_json::from_str(settings.expect("--settings")).unwrap();
                let deny = settings["permissions"]["deny"].as_array().unwrap();
                for rule in [
                    "read_file(*)",
                    "read_file(/)",
                    "write_file(*)",
                    "command(*)",
                    "mcp(*)",
                    "read_url(*)",
                    "execute_url(*)",
                    "download_url(*)",
                ] {
                    assert!(deny.iter().any(|r| r == rule), "{rule} not denied");
                }
                assert!(!argv.iter().any(|a| a.contains("dangerously")));
            }
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
        for unknown in ["gemini", "", "claude-x", "agy-x"] {
            assert!(review_vendor(unknown).is_err(), "{unknown} accepted");
        }
        assert_eq!(review_vendor(" Codex ").unwrap(), vendor("codex").unwrap());
        assert_eq!(
            review_vendor("antigravity").unwrap(),
            vendor("agy").unwrap()
        );
        // Only claude, with no tools, is verified unable to read local files;
        // a custom runner never is. trace:BUG-2783 | ai:claude
        assert!(!call(dir, &env).no_read());
        let confined = |name| {
            RunnerCall {
                agent: vendor(name),
                ..call(dir, &vars(&[]))
            }
            .no_read()
        };
        assert!(confined("claude") && !confined("codex") && !confined("agy"));
    }

    // trace:STORY-3120 | ai:claude
    #[test]
    fn s3b_runner_argv_carries_only_validated_tuning() {
        let (bin, checkout) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        script(bin.path(), "codex", DUMP);
        let path = format!("{}:/usr/bin:/bin", bin.path().display());
        let env = vars(&[("PATH", &path)]);
        let tuned = |agent: &str, effort| {
            let sources = [("--effort", Some(effort))];
            let tuning = super::super::review_tuning::resolve(
                vendor(agent),
                &sources,
                &[("--model", Some("o3"))],
            );
            RunnerCall {
                agent: vendor("codex"),
                tuning: tuning.unwrap(),
                ..call(checkout.path(), &env)
            }
        };
        let out = tuned("codex", "high").run("p").unwrap();
        let argv: Vec<_> = out.lines().filter_map(|l| l.strip_prefix("arg=")).collect();
        assert_eq!(
            argv,
            vendor("codex").unwrap().argv_with(Some("o3"), Some("high"))
        );
        assert!(
            out.contains("env=AIDA_REVIEW_RESOLVED_EFFORT=high"),
            "{out}"
        );
        // Tuning resolved for claude cannot carry its max level to codex.
        let err = tuned("claude", "max").run("p").unwrap_err().to_string();
        assert!(err.contains("no max effort"), "{err}");
        // Nor to a custom runner, which gets only the base levels.
        let custom = script(bin.path(), "custom", DUMP).display().to_string();
        let env = vars(&[("PATH", &path), (RUNNER_CMD, &custom)]);
        let runner = |effort| RunnerCall {
            parent_env: &env,
            ..tuned("claude", effort)
        };
        let err = runner("max").run("p").unwrap_err().to_string();
        assert!(
            err.contains("AIDA_REVIEW_RUNNER_CMD has no max effort"),
            "{err}"
        );
        let out = runner("xhigh").run("p").unwrap();
        assert!(
            out.contains("env=AIDA_REVIEW_RESOLVED_EFFORT=xhigh"),
            "{out}"
        );
    }

    // trace:STORY-3120 | ai:claude
    // Finding the escaped descendant reads /proc. trace:STORY-3120 | ai:claude
    #[cfg(target_os = "linux")]
    #[test]
    fn s3a2_runner_fails_closed_on_escaped_descendant() {
        let (bin, checkout) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        // The descendant leaves the process group and keeps stdout/stdin open; the
        // runner exits only once it is in its own session, so the kill misses it.
        let body = "#!/bin/sh\nsetsid sh -c 'touch \"$0.in\"; exec sleep 30' \"$0\" &\n\
            while [ ! -e \"$0.in\" ]; do sleep 0.05; done\nexit 0\n";
        let cmd = script(bin.path(), "runner", body).display().to_string();
        let env = vars(&[("PATH", "/usr/bin:/bin"), (RUNNER_CMD, &cmd)]);
        let started = Instant::now();
        let err = call(checkout.path(), &env)
            .run("p")
            .unwrap_err()
            .to_string();
        assert!(err.contains("left descendants holding its pipes"), "{err}");
        // trace:STORY-3120 | ai:claude
        let pid = err
            .split("escaped descendant pid ")
            .nth(1)
            .and_then(|rest| rest.split(|c: char| !c.is_ascii_digit()).next())
            .unwrap_or_else(|| panic!("no escaped pid reported: {err}"));
        assert!(err.contains("(killed)") && !err.contains("leaked"), "{err}");
        let gone = || {
            std::fs::read_to_string(format!("/proc/{pid}/stat")).map_or(true, |stat| {
                stat.rsplit(')')
                    .next()
                    .unwrap()
                    .trim_start()
                    .starts_with('Z')
            })
        };
        let wait = Instant::now() + Duration::from_secs(5);
        while !gone() && Instant::now() < wait {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(gone(), "escaped descendant {pid} still running");
        assert!(
            started.elapsed() < Duration::from_secs(11),
            "joined the descendant"
        );
    }

    /// agy takes `--effort` at its native levels; its print limit tracks the
    /// runner deadline and is never 0 (no limit).
    // trace:STORY-3202 | ai:claude
    #[test]
    fn story3202_agy_effort_mapping_and_print_timeout() {
        let agy = vendor("agy");
        for level in ["low", "medium", "high", "xhigh", "max"] {
            let effort = [("--effort", Some(level))];
            let tuning = super::super::review_tuning::resolve(agy, &effort, &[]).unwrap();
            let argv = agy.unwrap().argv_with(None, tuning.effort());
            assert!(
                argv.ends_with(&["--effort".into(), level.into()]),
                "{argv:?}"
            );
        }
        let effort = [("--effort", Some("extreme"))];
        assert!(super::super::review_tuning::resolve(agy, &effort, &[]).is_err());
        let secs = [600, 20, 0].map(|s| print_timeout(Duration::from_secs(s)));
        assert_eq!(secs, [570, 15, 1]);
    }

    /// A keyring-only agy in a lane (no session bus, no TTY) fails at once with
    /// a typed `AuthRequired`, not a hang or a bare exit status; a vendor
    /// without auth markers keeps the plain failure.
    // trace:STORY-3202 | ai:claude
    #[test]
    fn story3202_agy_auth_failure_is_a_typed_cause() {
        let (bin, checkout) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let body = "#!/bin/sh\ncat >/dev/null\necho 'W composite_token_storage: Failed to load \
            stored token from keyring' >&2\necho 'error: Authentication failed or timed out' >&2\nexit 1\n";
        script(bin.path(), "agy", body);
        script(bin.path(), "claude", body);
        let path = format!("{}:/usr/bin:/bin", bin.path().display());
        let env = vars(&[("PATH", &path)]);
        let run = |agent| {
            let started = Instant::now();
            let err = RunnerCall {
                agent: vendor(agent),
                deadline: Duration::from_secs(30),
                ..call(checkout.path(), &env)
            }
            .run("p")
            .unwrap_err();
            assert!(started.elapsed() < Duration::from_secs(10), "{err:#}");
            err
        };
        let err = run("agy");
        let cause = err
            .downcast_ref::<AuthRequired>()
            .expect("typed auth cause");
        assert_eq!(cause.vendor, "agy");
        let text = err.to_string();
        assert!(
            text.contains("not signed in") && text.contains("--lane-host"),
            "{text}"
        );
        let plain = run("claude");
        assert!(plain.downcast_ref::<AuthRequired>().is_none(), "{plain:#}");
        assert!(
            plain.to_string().contains("review runner failed"),
            "{plain:#}"
        );
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

    /// A runner that reads stdin to EOF and then waits on an answer gets EOF at
    /// once; one that prompts on the terminal fails at once naming the cause,
    /// both far inside the deadline.
    // trace:BUG-2775 | ai:claude
    #[test]
    fn bug2775_runner_gets_stdin_eof_and_a_prompting_cli_fails_fast() {
        let (bin, checkout) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let run = |body: &str| {
            let cmd = script(bin.path(), "runner", body).display().to_string();
            let env = vars(&[("PATH", "/usr/bin:/bin"), (RUNNER_CMD, &cmd)]);
            let runner = RunnerCall {
                deadline: Duration::from_secs(30),
                ..call(checkout.path(), &env)
            };
            let started = Instant::now();
            let res = runner.run("the prompt");
            assert!(started.elapsed() < Duration::from_secs(10), "{res:?}");
            res
        };
        let asks = "#!/bin/sh\nn=$(wc -c)\nread answer && exit 9\necho \"read=$n eof\"\n";
        assert_eq!(run(asks).unwrap().trim(), "read=10 eof");
        let tty = "#!/bin/sh\ncat >/dev/null\nread answer </dev/tty || exit 5\necho answered\n";
        let err = format!("{:#}", run(tty).unwrap_err());
        assert!(
            err.contains("failed (exit status") && err.contains("tty"),
            "{err}"
        );
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
