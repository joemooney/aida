//! Host remote calls (store sync, push/pull, doctor, PR, session and phase
//! probes) keep an operator-embedded `oauth2:<token>@` origin URL out of
//! `git-remote-https` argv, and still authenticate with that token.
//!
//! The fixture origin is a credentialed `https://` URL. A fake
//! `git-remote-https` on `GIT_EXEC_PATH` records its argv, refuses unless
//! `git credential fill` yields the token, then proxies a real transfer to a
//! local bare repository through `git remote-ext`.
// trace:BUG-2181 | ai:claude

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

const SECRET: &str = "glpat-host-remote-secret-2181";
const HOST: &str = "fixture.example.test";

struct CredentialedOrigin {
    _tmp: TempDir,
    repo: PathBuf,
    bare: PathBuf,
    argv_log: PathBuf,
    exec_dir: PathBuf,
    main_sha: String,
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = crate::test_git::git_command()
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

impl CredentialedOrigin {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let bare = tmp.path().join("bare.git");
        let seed = tmp.path().join("seed");
        let repo = tmp.path().join("repo");
        git(
            tmp.path(),
            &["init", "-q", "--bare", bare.to_str().unwrap()],
        );
        git(
            tmp.path(),
            &["init", "-q", "-b", "main", seed.to_str().unwrap()],
        );
        git(&seed, &["commit", "-q", "--allow-empty", "-m", "seed"]);
        git(&seed, &["push", "-q", bare.to_str().unwrap(), "main"]);
        let main_sha = git(&seed, &["rev-parse", "HEAD"]);
        git(
            tmp.path(),
            &["init", "-q", "-b", "work", repo.to_str().unwrap()],
        );
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "work"]);
        let url = format!("https://oauth2:{SECRET}@{HOST}/group/project.git");
        git(&repo, &["remote", "add", "origin", &url]);

        let exec_dir = tmp.path().join("git-exec");
        fs::create_dir_all(&exec_dir).unwrap();
        let argv_log = tmp.path().join("remote_https_argv.txt");
        crate::test_exec::write_executable(
            &exec_dir.join("git-remote-https"),
            format!(
                "#!/bin/sh\n\
                 for a in \"$@\"; do printf '%s\\n' \"$a\" >> '{log}'; done\n\
                 cred=$(printf 'protocol=https\\nhost={HOST}\\n\\n' | GIT_TERMINAL_PROMPT=0 git credential fill 2>/dev/null)\n\
                 case \"$cred\" in *'password={SECRET}'*) ;; *) echo 'fatal: Authentication failed' >&2; exit 128 ;; esac\n\
                 exec git -c protocol.ext.allow=always remote-ext \"$1\" \"git %s {bare}\"\n",
                log = argv_log.display(),
                bare = bare.display(),
            ),
        );
        CredentialedOrigin {
            _tmp: tmp,
            repo,
            bare,
            argv_log,
            exec_dir,
            main_sha,
        }
    }

    /// Route every `git` child's https transport through the fake helper.
    fn transport_env(&self) -> crate::test_env::EnvVarsGuard {
        let real_exec = git(&self.repo, &["--exec-path"]);
        let path = format!(
            "{}:{real_exec}:{}",
            self.exec_dir.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        crate::test_env::EnvVarsGuard::apply(&[
            ("GIT_EXEC_PATH", Some(self.exec_dir.to_str().unwrap())),
            ("PATH", Some(&path)),
            ("GIT_TERMINAL_PROMPT", Some("0")),
        ])
    }

    fn argv(&self) -> String {
        fs::read_to_string(&self.argv_log).unwrap_or_default()
    }
}

/// Each routed call reaches the credentialed origin, authenticates with the
/// stored token, and the transport helper's argv only ever names the clean
/// URL. Covers the core `git_ops` remote calls (store sync's
/// `remote_branch_head_sha`, fetch, push) and the cli probes.
#[cfg(unix)]
#[test]
fn routed_host_remote_calls_authenticate_without_the_token_in_argv() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let fx = CredentialedOrigin::new();
    let _env = fx.transport_env();
    let repo = fx.repo.as_path();

    assert_eq!(
        aida_core::git_ops::remote_branch_head_sha(repo, "origin", "main").as_deref(),
        Some(fx.main_sha.as_str()),
        "store sync's ls-remote must authenticate and read origin/main"
    );
    assert!(aida_core::git_ops::is_remote_reachable(repo, "origin"));
    aida_core::git_ops::fetch_branch_into_local(repo, "origin", "main").unwrap();
    assert!(
        aida_core::git_ops::push(repo, "origin", "work").unwrap(),
        "push must authenticate"
    );
    assert_eq!(
        git(&fx.bare, &["rev-parse", "refs/heads/work"]),
        git(repo, &["rev-parse", "work"]),
        "the push reached origin"
    );
    assert_eq!(
        crate::phase_driver::remote_head_sha(repo, "main").as_deref(),
        Some(fx.main_sha.as_str())
    );
    assert_eq!(
        crate::orchestrator_leases::remote_branch_gone(repo, "main"),
        Some(false)
    );
    assert_eq!(
        crate::orchestrator_leases::remote_branch_gone(repo, "no-such-branch"),
        Some(true)
    );
    assert!(matches!(
        crate::pr_lookup::probe_branch_on_origin(repo, "main"),
        crate::pr_lookup::BranchOriginProbe::Present
    ));
    let fetched = crate::store_push_pull::run_remote_git_with_timeout(
        repo,
        "origin",
        &["fetch", "--quiet", "--no-tags", "origin", "main"],
        std::time::Duration::from_secs(60),
    )
    .unwrap();
    assert!(fetched.success(), "timed fetch must authenticate");
    crate::store_push_pull::fetch_branch(repo, "main", true).unwrap();

    let argv = fx.argv();
    assert!(
        !argv.contains(SECRET),
        "the origin token must never reach git-remote-https argv:\n{argv}"
    );
    assert!(
        argv.contains(&format!("https://{HOST}/group/project.git")),
        "git-remote-https must receive the clean origin URL:\n{argv}"
    );
    assert_eq!(
        git(repo, &["config", "--get", "remote.origin.url"]),
        format!("https://oauth2:{SECRET}@{HOST}/group/project.git"),
        "the operator's stored credential must be left untouched"
    );
}

/// The seam is live: a bare `git ls-remote origin` that skips the helper
/// does put the token in the transport's argv, so the test above can fail.
#[cfg(unix)]
#[test]
fn an_unrouted_remote_call_leaks_the_token_into_transport_argv() {
    if crate::test_env::run_path_isolated_test() {
        return;
    }
    let fx = CredentialedOrigin::new();
    let _env = fx.transport_env();
    let _ = crate::test_git::git_command()
        .arg("-C")
        .arg(&fx.repo)
        .args(["ls-remote", "origin"])
        .output()
        .unwrap();
    assert!(
        fx.argv().contains(SECRET),
        "an unrouted call must expose the token, or the argv seam is vacuous:\n{}",
        fx.argv()
    );
}

/// TASK-2001: the host git builders in `src` (1-based lines) that contact a
/// remote (`fetch`, `pull`, `push` or `ls-remote` as the subcommand, after any
/// `-c k=v` options, or held in a local array passed as `.args(name)`; a
/// `let mut` builder in any statement up to its scope end)
/// with no credential routing (`remote_credentials`, `host_remote_git_command`)
/// in the builder's own chain, and no
/// `remote-creds-ok(TASK-2001): <why>` comment in the three lines above. A builder is `host_git_command(`, any
/// `<Type>::new("git")`, or a `git(dir, &["fetch", ...])` style helper call.
/// Inline `#[cfg(test)] mod name { ... }` blocks are skipped.
// trace:TASK-1952 | trace:TASK-2001 | ai:claude
fn task_2001_unrouted_remote_calls(src: &str) -> Vec<usize> {
    use std::sync::LazyLock;
    // Compiled once per process: the ratchet scans every workspace source.
    static BUILDER: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r#"host_git_command\(|\b\w+::new\("git"\)"#).unwrap());
    static VERB: LazyLock<regex::Regex> = LazyLock::new(|| {
        // PR-2837 review: global options may precede the subcommand
        // (`"-c", "http.lowSpeedLimit=1000", "ls-remote"`), so skip any run of
        // `"-c"`/`"-C"` with its value and of other `"-..."` literals first.
        let opts = r#"(?:"-[cC]"\s*,\s*[^,\]]+,\s*|"-[^"]*"\s*,\s*)*"#;
        regex::Regex::new(&format!(
            r#"\.args?\(\s*&?\[?\s*{opts}"(fetch|pull|push|ls-remote)"|\bgit\(\s*[^,()]+,\s*&\[\s*{opts}"(fetch|pull|push|ls-remote)""#
        ))
        .unwrap()
    });
    static LET_MUT: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\blet mut (\w+)").unwrap());
    static ROUTED: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"remote_credentials|host_remote_git_command").unwrap());
    static MODULE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"^\s*(pub(\(crate\))? )?mod \w+\s*\{").unwrap());
    static NEXT_FN: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"^\s*(pub(\([^)]*\))? )?(async )?fn \w+").unwrap());
    static ARGS_IDENT: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\.args\(\s*&?(\w+)\s*\)").unwrap());
    static REMOTE_PARAM: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\bremote\s*:").unwrap());
    static TRAILING_COMMENT: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"(^|\s)//.*$").unwrap());
    // A file naming no remote verb and no `remote` parameter has nothing to
    // check.
    if ![
        "\"fetch\"",
        "\"pull\"",
        "\"push\"",
        "\"ls-remote\"",
        "remote:",
    ]
    .iter()
    .any(|v| src.contains(v))
    {
        return Vec::new();
    }
    let (builder, verb, routed, module, next_fn) =
        (&*BUILDER, &*VERB, &*ROUTED, &*MODULE, &*NEXT_FN);
    let lines: Vec<&str> = src.lines().collect();
    let mut tests = std::collections::HashSet::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() == "#[cfg(test)]" && lines.get(i + 1).is_some_and(|l| module.is_match(l))
        {
            let mut j = i + 1;
            while j < lines.len() && lines[j] != "}" {
                tests.insert(j);
                j += 1;
            }
            i = j;
        }
        i += 1;
    }
    let mut found = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if tests.contains(&i) || line.trim_start().starts_with("//") {
            continue;
        }
        let helper_call = verb.captures(line).is_some_and(|c| c.get(2).is_some());
        if !builder.is_match(line) && !helper_call {
            continue;
        }
        // BUG review (PR-2837): comments are not code; a `;` inside one must
        // not end the builder chain before its `.args(["push", ...])`.
        let strip = |ls: &[&str]| {
            ls.iter()
                .map(|l| TRAILING_COMMENT.replace(l, "").into_owned())
                .collect::<Vec<_>>()
                .join("\n")
        };
        let chain = if let Some(var) = LET_MUT.captures(line).map(|c| c[1].to_string()) {
            // PR-2837 review: a `let mut` builder may take its verb any
            // number of statements later, so scan every statement naming the
            // variable up to the end of its block or its next re-binding.
            let mention = regex::Regex::new(&format!(r"\b{var}\b")).unwrap();
            let rebind = regex::Regex::new(&format!(r"\blet (mut )?{var}\b")).unwrap();
            let mut depth = 0i64;
            let mut end = lines.len();
            for (j, l) in lines.iter().enumerate().skip(i) {
                let code = TRAILING_COMMENT.replace(l, "");
                if j > i && rebind.is_match(&code) {
                    end = j;
                    break;
                }
                depth += code.matches('{').count() as i64 - code.matches('}').count() as i64;
                if depth < 0 {
                    end = j + 1;
                    break;
                }
            }
            strip(&lines[i..end])
                .split(';')
                .filter(|st| mention.is_match(st))
                .collect::<Vec<_>>()
                .join(";")
        } else {
            // PR-2837 review round 3: the statement runs to its `;` at
            // bracket depth 0 (or the close of the enclosing block, for a
            // tail expression), however many lines the chain spans. The
            // scan starts at the builder, so a one-line `fn f() { .. }`
            // around it does not extend the statement into the next fn.
            let text = strip(&lines[i..]);
            let first = TRAILING_COMMENT.replace(line, "");
            let from = builder
                .find(&first)
                .or_else(|| verb.find(&first))
                .map_or(0, |m| m.start());
            let text = &text[from..];
            let mut depth = 0i64;
            let end = text
                .char_indices()
                .find(|&(_, c)| {
                    match c {
                        '(' | '[' | '{' => depth += 1,
                        ')' | ']' | '}' => depth -= 1,
                        _ => {}
                    }
                    depth < 0 || (c == ';' && depth == 0)
                })
                .map_or(text.len(), |(at, _)| at);
            text[..end].to_string()
        };
        // TASK-2245: a verb held in a local array (`let args = ["push", ..];
        // .args(args)`) is resolved from its binding earlier in the same fn.
        let resolved = ARGS_IDENT.captures_iter(&chain).find_map(|c| {
            let name = &c[1];
            let fn_start = (0..=i)
                .rev()
                .find(|&j| next_fn.is_match(lines[j]))
                .unwrap_or(0);
            let binding =
                regex::Regex::new(&format!(r"\blet (mut )?{name}\s*(:[^=]*)?=\s*&?(vec!)?\["))
                    .unwrap();
            let at = (fn_start..i).rev().find(|&j| binding.is_match(lines[j]))?;
            let body = strip(&lines[at..i]);
            let open = binding.find(&body)?.end();
            let close = body[open..].find(']')?;
            Some(format!(".args([{}])", &body[open..open + close]))
        });
        let chain = match resolved {
            Some(args) => format!("{chain}\n{args}"),
            None => chain,
        };
        if !verb.is_match(&chain) {
            continue;
        }
        // PR-2837 review round 3: the routing must sit in the builder's OWN
        // chain (up to its statement's `;`, or for a `let mut` builder, the
        // statements naming it), so a neighbouring call's routing cannot mask
        // a dropped one.
        let justified = lines[i.saturating_sub(3)..i]
            .iter()
            .any(|l| l.contains("remote-creds-ok(TASK-2001):"));
        if !routed.is_match(&chain) && !justified {
            found.push(i + 1);
        }
    }
    // A wrapper that takes the remote as a parameter takes its subcommand
    // from the caller too (`git_push_in(dir, remote, args)`), so no verb is
    // visible here: any git builder in a fn with a `remote` parameter must
    // route that remote's credentials somewhere in its body.
    let starts: Vec<usize> = (0..lines.len())
        .filter(|&i| !tests.contains(&i) && next_fn.is_match(lines[i]))
        .collect();
    for (n, &start) in starts.iter().enumerate() {
        let end = starts.get(n + 1).copied().unwrap_or(lines.len());
        let sig_end = (start..end.min(start + 10))
            .find(|&j| lines[j].contains('{'))
            .unwrap_or(start);
        let signature = lines[start..=sig_end].join(" ");
        if !REMOTE_PARAM.is_match(&signature) {
            continue;
        }
        let body: Vec<&str> = lines[start..end]
            .iter()
            .copied()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect();
        let body = body.join("\n");
        let justified = lines[start.saturating_sub(3)..start]
            .iter()
            .any(|l| l.contains("remote-creds-ok(TASK-2001):"));
        if builder.is_match(&body) && !routed.is_match(&body) && !justified {
            found.push(start + 1);
        }
    }
    found.sort_unstable();
    found.dedup();
    found
}

// TASK-2001 ratchet: a later refactor that drops the credential routing from
// any host remote call fails here, not silently in production. Scans the
// non-test source of every workspace crate.
// trace:TASK-1952 | trace:TASK-2001 | ai:claude
#[test]
fn task_2001_every_host_remote_call_routes_its_credentials() {
    let sample = "fn a(d: &Path) {\n    let _ = Command::new(\"git\")\n        .arg(\"-C\")\n        \
                  .arg(d)\n        .args([\"fetch\", \"origin\"])\n        .output();\n}\n\
                  fn b(d: &Path) {\n    let _ = host_git_command(d)\n        .args([\"push\", \"origin\", \"x\"])\n        \
                  .remote_credentials(d, \"origin\")\n        .output();\n}\n\
                  fn c(d: &Path) {\n    let mut cmd = host_git_command(d);\n    cmd.args([\"ls-remote\", \"origin\"]);\n    \
                  configure_remote_credentials(&mut cmd, d, \"origin\");\n}\n\
                  fn e(d: &Path) { let _ = PCmd::new(\"git\").arg(\"pull\").output(); }\n\
                  fn f(d: &Path) { let _ = git(d, &[\"fetch\", \"origin\"]); }\n\
                  fn g(d: &Path) { let _ = Command::new(\"git\").args([\"stash\", \"push\"]).output(); }\n\
                  fn h(d: &Path) {\n    // remote-creds-ok(TASK-2001): a local path remote.\n    \
                  let _ = Command::new(\"git\").args([\"fetch\", \"../x\"]).output();\n}\n\
                  #[cfg(test)]\nmod t {\n    fn z(d: &Path) { Command::new(\"git\").args([\"push\"]); }\n}\n";
    assert_eq!(task_2001_unrouted_remote_calls(sample), vec![2, 19, 20]);
    // PR-2837 review: a comment holding `;` between the builder and its
    // verb (doctor heal's `push --delete`).
    let commented =
        "fn heal(d: &Path) {\n    let _ = Command::new(\"git\")\n        .arg(d)\n        \
                     // The id is a branch name; end options before it.\n        \
                     .args([\"push\", \"--delete\", \"origin\", \"x\"])\n        .status();\n}\n";
    assert_eq!(task_2001_unrouted_remote_calls(commented), vec![2]);
    // Parameter-driven wrappers: the verb comes from the caller.
    let wrappers = "fn git_push_in(dir: &Path, remote: &str, args: &[&str]) {\n    \
                    let _ = Command::new(\"git\").args(args).current_dir(dir).status();\n}\n\
                    fn git_raw_via(\n    repo: &Path,\n    remote: Option<&str>,\n    args: &[&str],\n) {\n    \
                    let mut cmd = Command::new(\"git\");\n    cmd.arg(\"-C\").arg(repo).args(args);\n}\n\
                    fn remote_git_with_env(cwd: &Path, remote: &str, args: &[&str]) {\n    \
                    run(host_git_command(cwd), args);\n}\n\
                    fn routed(dir: &Path, remote: &str, args: &[&str]) {\n    \
                    let _ = Command::new(\"git\").args(args).remote_credentials(dir, remote).status();\n}\n\
                    fn via_helper(cwd: &Path, remote: &str, args: &[&str]) {\n    \
                    run(host_remote_git_command(cwd, remote), args);\n}\n";
    assert_eq!(task_2001_unrouted_remote_calls(wrappers), vec![1, 4, 12]);
    // PR-2837 review: `-c key=value` options before the verb (pr_lookup's
    // ls-remote probe, the mirror push) with their routing removed.
    let leading_options = "fn probe(d: &Path) {\n    let out = std::process::Command::new(\"git\")\n        \
                           .current_dir(d)\n        .args([\n            \"-c\",\n            \
                           // Keep the probe bounded.\n            \"http.lowSpeedLimit=1000\",\n            \
                           \"-c\",\n            \"http.lowSpeedTime=10\",\n            \"ls-remote\",\n            \
                           \"origin\",\n        ])\n        .output();\n}\n\
                           fn mirror(d: &Path, m: &str) {\n    let mut command = Command::new(\"git\");\n    \
                           command.arg(\"-C\").arg(d).args([\n        \"-c\",\n        \
                           \"core.hooksPath=/dev/null\",\n        \"push\",\n    ]);\n    \
                           let _ = command.arg(m).output();\n}\n\
                           fn stash(d: &Path) { let _ = Command::new(\"git\").args([\"-c\", \"x=y\", \"stash\", \"push\"]).output(); }\n";
    assert_eq!(
        task_2001_unrouted_remote_calls(leading_options),
        vec![2, 16]
    );
    // PR-2837 review: a `let mut` builder whose verb sits in its 4th+
    // statement (remote_create's mirror-sync push_cmd), routing removed;
    // a re-bound name ends the scan.
    let long_chain = "fn sync(d: &Path, m: &str) {\n    let mut push_cmd = Command::new(\"git\");\n    \
                      push_cmd.arg(\"-C\").arg(d);\n    // a; b; c\n    push_cmd.env(\"A\", \"1\");\n    \
                      push_cmd.env(\"B\", \"2\");\n    let out = push_cmd\n        .args([\"push\", m])\n        \
                      .output();\n}\n\
                      fn local(d: &Path) {\n    let mut cmd = Command::new(\"git\");\n    cmd.arg(\"status\");\n    \
                      let _ = cmd.output();\n    let mut cmd = host_git_command(d);\n    \
                      cmd.args([\"fetch\"]).remote_credentials(d, \"origin\");\n}\n";
    assert_eq!(task_2001_unrouted_remote_calls(long_chain), vec![2]);
    // PR-2837 review round 3: two adjacent builders (pr_cmd's preflight
    // `fetch --prune` then the PR-head fetch) where only the second routes;
    // its routing must not mask the first, in either builder shape.
    let adjacent = "fn preflight(d: &Path, n: u32) {\n    let _ = host_git_command(d)\n        \
                    .args([\"fetch\", \"origin\", \"--prune\"])\n        .output();\n    \
                    let _ = host_git_command(d)\n        .args([\"fetch\", \"origin\", &n.to_string()])\n        \
                    .remote_credentials(d, \"origin\")\n        .output();\n}\n\
                    fn heads(d: &Path) {\n    let mut a = Command::new(\"git\");\n    a.args([\"fetch\", \"origin\"]);\n    \
                    let mut b = Command::new(\"git\");\n    b.args([\"fetch\", \"upstream\"]);\n    \
                    configure_remote_credentials(&mut b, d, \"upstream\");\n}\n";
    assert_eq!(task_2001_unrouted_remote_calls(adjacent), vec![2, 11]);
    // TASK-2245: the verb held in a local array (queue recover's push),
    // routing removed; the routed twin and a non-remote array pass.
    let local_array = "fn recover(repo: &Path, b: &str) {\n    let args = [\n        \"push\",\n        \
                       \"-u\",\n        \"origin\",\n        b,\n    ];\n    \
                       let _ = std::process::Command::new(\"git\")\n        .arg(\"-C\")\n        .arg(repo)\n        \
                       .args(args)\n        .status();\n}\n\
                       fn routed(repo: &Path) {\n    let args = [\"fetch\", \"origin\"];\n    \
                       let _ = Command::new(\"git\").args(args).remote_credentials(repo, \"origin\").status();\n}\n\
                       fn local(repo: &Path) {\n    let args = [\"status\", \"--short\"];\n    \
                       let _ = Command::new(\"git\").args(args).status();\n}\n";
    assert_eq!(task_2001_unrouted_remote_calls(local_array), vec![8]);
    // The own chain runs to the statement's `;` however long it is
    // (preflight_mr_base's 17-line ls-remote), and stops there.
    let tall = format!(
        "fn tall(d: &Path) {{\n    let _ = Command::new(\"git\")\n        .args([\"ls-remote\",\n{}        ])\n        \
         .remote_credentials(d, \"origin\")\n        .output();\n    let _ = Command::new(\"git\").args([\"push\"]).status();\n}}\n",
        "            \"x\",\n".repeat(16)
    );
    assert_eq!(task_2001_unrouted_remote_calls(&tall), vec![23]);

    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut offenders = Vec::new();
    let mut stack: Vec<PathBuf> = fs::read_dir(workspace)
        .unwrap()
        .flatten()
        .map(|e| e.path().join("src"))
        .filter(|src| src.is_dir() && src.parent().unwrap().join("Cargo.toml").is_file())
        .collect();
    assert!(stack.contains(&workspace.join("aida-core").join("src")));
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir() {
                if name != "tests" {
                    stack.push(path);
                }
                continue;
            }
            if !name.ends_with(".rs") || name.ends_with("_tests.rs") || name == "tests.rs" {
                continue;
            }
            let src = fs::read_to_string(&path).unwrap();
            for line in task_2001_unrouted_remote_calls(&src) {
                let rel = path.strip_prefix(workspace).unwrap_or(&path);
                offenders.push(format!("{}:{line}", rel.display()));
            }
        }
    }
    offenders.sort();
    assert!(
        offenders.is_empty(),
        "a host git call contacts a remote without credential routing; add \
         `.remote_credentials(dir, remote)` (or `configure_remote_credentials`), or justify \
         it with `// remote-creds-ok(TASK-2001): <why>`: {offenders:?}"
    );
}
