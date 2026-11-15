/// BUG-226: spawn (not exec) a headless `claude -p` reviewer and wait,
/// returning the exit status. Mirrors `exec_claude_headless` exactly —
/// same `claude_headless_args_with_posture` flag set, `AIDA_HEADLESS=1` in the env,
/// stdout redirected to `log_path` — but keeps the parent alive so the
/// standalone reviewer summary can read the verdict file + JSONL log.
/// trace:BUG-226 | ai:claude
///
/// TASK-307: starts a background tee thread on `log_path` so the headless
/// session's high-signal events surface to the orchestrator's terminal
/// alongside the launch banner. The tee's filter and on/off are driven by
/// `tee_opts`; even when disabled, failure events (`is_error`,
/// `permission_denials`) still stream. trace:TASK-307 | ai:claude
pub fn spawn_claude_headless(
    prompt: &str,
    session_id: &str,
    log_path: &Path,
    tee_opts: &crate::headless_tee::TeeOptions,
    contained: bool,
) -> Result<std::process::ExitStatus> {
    // STORY-683: resolve the vendor (default Claude) and delegate. Keeping the
    // `spawn_claude_headless` name + signature means every existing call site is
    // unchanged and an un-configured drain is byte-identical to before — the
    // vendor only diverges when `AIDA_HEADLESS_VENDOR` / `[orchestrator]
    // headless_vendor` selects Codex. trace:STORY-683 | ai:claude
    let vendor = resolve_headless_vendor(&headless_worktree_root());
    spawn_vendor_headless(vendor, prompt, session_id, log_path, tee_opts, contained)
}

/// The vendor and seat a one-shot headless analysis pass (`dryrun --ai`,
/// `intent`, `questions research`) launches with: the configured vendor, on
/// the implementer seat these passes have always run under. The seat decides
/// which delegation the launcher's grant must carry, so it does not change
/// with the vendor.
// trace:BUG-2389 | ai:claude
pub(crate) fn analysis_headless_launch(
    project_root: &Path,
) -> (HeadlessVendor, aida_core::agents_config::AgentSeat) {
    (
        resolve_headless_vendor(project_root),
        aida_core::agents_config::AgentSeat::Implementer,
    )
}

/// Spawn a headless analysis pass on [`analysis_headless_launch`]'s vendor
/// and seat, contained when the parent is or the project/user configuration
/// asks for it ([`drain_contained`]).
// trace:BUG-2389 BUG-2393 | ai:claude
pub(crate) fn spawn_analysis_headless(
    project_root: &Path,
    prompt: &str,
    session_id: &str,
    log_path: &Path,
    tee_opts: &crate::headless_tee::TeeOptions,
) -> Result<std::process::ExitStatus> {
    let (vendor, seat) = analysis_headless_launch(project_root);
    // BUG-2393: configured containment (`[agents] contained`, `[contained]
    // enable`) applies even when `AIDA_CONTAINED` is not in the environment.
    // trace:BUG-2393 | ai:claude
    let contained = drain_contained(project_root)?;
    spawn_vendor_headless_with_seat(
        vendor, seat, prompt, session_id, log_path, tee_opts, contained,
    )
}

/// Compose the (program, argv) for a headless run of `vendor`'s CLI — the ONE
/// place both the spawn path ([`spawn_vendor_headless`]) and the exec path
/// ([`exec_claude_headless`], the `--no-human` phase-1 implementer) build the
/// command, so a vendor knob can never route one and miss the other again.
/// Applies the opt-in OS-boundary wrapper (`bwrap`, STORY-612) around whichever
/// program, and routes the binary through the `AIDA_AGENT_CMD` mock resolver
/// (TASK-1081) before wrapping.
// trace:BUG-705 STORY-683 STORY-612 TASK-1081 | ai:claude
/// BUG-799: expand an `/aida-<name> [args]` slash-command into the rendered
/// Codex prompt body — because `codex exec` does NOT expand custom prompts the
/// way an interactive `/name` invocation does, and the way `claude -p` expands
/// skills (SPIKE-51). Without this, every drain phase on Codex handed the
/// model a literal slash string; the model reconstructed the workflow by
/// searching the repo, and contracts that are not discoverable from context —
/// the reviewer's verdict-file handshake — were dropped. Proven twice on live
/// drains (PR #1610, PR #1612) before this fix.
///
/// The ORIGINAL invocation line is appended under the body whenever it carries
/// arguments the substitution may not have placed (a body with no
/// `$ARGUMENTS` placeholder would otherwise silently eat `--pr 1612`, and the
/// PR number reaching the model is load-bearing).
///
/// `None` — caller keeps the raw prompt — for non-slash prompts, non-aida
/// commands, and the non-portable set: degraded exactly as today, never worse.
// trace:BUG-799 | ai:claude
pub(crate) fn codex_inline_prompt(prompt: &str) -> Option<String> {
    let rest = prompt.strip_prefix('/')?;
    let (name, args) = match rest.split_once(char::is_whitespace) {
        Some((n, a)) => (n, a.trim()),
        None => (rest, ""),
    };
    if !name.starts_with("aida-") {
        return None;
    }
    let body = aida_core::scaffolding::codex_prompts::render_codex_command_prompt(name, args)?;
    if args.is_empty() || body.contains(args) {
        Some(body)
    } else {
        Some(format!("{body}\n\n---\nInvocation: {prompt}"))
    }
}

/// Render the ONE interactive AIDA invocation line of a Codex launch prompt.
///
/// Only the intended invocation is expanded: the first line, or — when the
/// prompt opens with the agent launch-context preamble — the first `/aida-*`
/// line inside that preamble paragraph (e.g. `aida derisk`). Every other line,
/// including pasted usage blocks such as `/aida-review --pr 7   Explicit PR
/// number` later in free-form `--prompt` text, stays verbatim so several
/// workflow bodies are never inlined mid-prompt.
///
/// Commands without a portable embedded body use Codex's installed-skill syntax;
/// the explicit stop instruction prevents an unavailable skill being invented.
// trace:BUG-2387 | ai:claude
pub(crate) fn codex_interactive_prompt(prompt: &str) -> String {
    let mut lines: Vec<&str> = prompt.split('\n').collect();
    let target = if lines[0].starts_with("/aida-") {
        Some(0)
    } else if lines[0] == crate::agent_launch_prompt::LAUNCH_CONTEXT_PREAMBLE {
        lines
            .iter()
            .take_while(|line| !line.trim().is_empty())
            .position(|line| line.starts_with("/aida-"))
    } else {
        None
    };
    let Some(index) = target else {
        return prompt.to_string();
    };
    let line = lines[index];
    let rendered = codex_inline_prompt(line).unwrap_or_else(|| {
        format!(
            "${}\n\nUse the installed Codex skill requested above. \
             If it is unavailable, stop and report that the skill is unavailable; \
             do not reconstruct or claim to have run its workflow.",
            &line[1..]
        )
    });
    lines[index] = &rendered;
    lines.join("\n")
}

/// BUG-806: the PATH a phase session should run with — the directory of the
/// binary that is ORCHESTRATING, prepended to the inherited PATH. Without
/// this, a headless agent's login shell resolves `aida` however the machine's
/// profile does (on the dev box: a months-old ~/.local/bin install), and the
/// agent ends up holding older tools than the contract its prompt describes.
/// Observed live: a reviewer that correctly ran `review record --pr` against
/// a stale binary that did not have the flag.
// trace:BUG-806 | ai:claude
pub(crate) fn drive_path_env() -> Option<std::ffi::OsString> {
    let exe_dir = crate::aida_exe_path().parent()?.to_path_buf();
    let old = std::env::var_os("PATH").unwrap_or_default();
    let mut parts = vec![exe_dir];
    parts.extend(std::env::split_paths(&old));
    std::env::join_paths(parts).ok()
}

/// `host_main` is the main checkout the launch's entry point resolved before
/// any chdir into a seat (TASK-2032): every builder below takes it
/// explicitly and never derives it from the process cwd.
// trace:TASK-2032 | ai:claude
pub(crate) fn compose_headless_command(
    host_main: Option<&Path>,
    vendor: HeadlessVendor,
    prompt: &str,
    session_id: &str,
    contained: bool,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<(String, Vec<String>)> {
    compose_headless_command_forking(
        host_main, vendor, prompt, session_id, contained, model, effort, None,
    )
}

/// [`compose_headless_command`], optionally forking the Codex thread
/// `fork_thread` (`codex exec fork <thread> <prompt>`) instead of a fresh
/// `codex exec`. The fork reuses the exact posture the fresh launch would
/// get, so a contained fork carries the same flags and no bypass flag.
/// `Some` on a non-Codex vendor is an error.
// trace:STORY-2255 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn compose_headless_command_forking(
    host_main: Option<&Path>,
    vendor: HeadlessVendor,
    prompt: &str,
    session_id: &str,
    contained: bool,
    model: Option<&str>,
    effort: Option<&str>,
    fork_thread: Option<&str>,
) -> Result<(String, Vec<String>)> {
    anyhow::ensure!(
        fork_thread.is_none() || vendor == HeadlessVendor::Codex,
        "only a codex thread can be forked with `exec fork`, not {}",
        vendor.as_str()
    );
    // trace:BUG-2172 | ai:codex
    let contained = aida_contained_env()? || contained;
    // BUG-2144: fail closed before composing anything. Every headless launch
    // (the queue-work implementer, the headless reviewer, harvest, …) builds
    // its argv here, so a contained request on a vendor that cannot apply it
    // is refused at the one choke point instead of silently running uncontained.
    // trace:BUG-2144 | ai:claude
    require_vendor_containment(vendor, contained, "headless session")?;
    // trace:TASK-2631 | ai:codex
    if matches!(
        containment_posture(vendor, contained),
        Ok(ContainmentPosture::Native)
    ) {
        eprintln!(
            "  {}: {}",
            vendor.as_str(),
            ContainmentPosture::Native.label()
        );
    }
    let worktree = headless_worktree_root();
    // BUG-049: a fresh seat worktree is untrusted, so Claude Code would drop
    // the project allow-list and deny the lane's Bash calls. Pre-trust it
    // before the spawn — Claude only; opt-in; seats under the worktrees root
    // only. Best-effort: a failure warns and the launch goes ahead.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-049 | ai:claude
    if vendor == HeadlessVendor::Claude {
        crate::claude_trust::report(&crate::claude_trust::ensure_seat_trusted(&worktree));
    }
    // BUG-799: Codex gets slash-commands inline-rendered; Claude expands its
    // own skills, so its path is byte-identical to before.
    let effective_prompt: std::borrow::Cow<'_, str> = if vendor == HeadlessVendor::Codex {
        match codex_inline_prompt(prompt) {
            Some(expanded) => std::borrow::Cow::Owned(expanded),
            None => std::borrow::Cow::Borrowed(prompt),
        }
    } else {
        std::borrow::Cow::Borrowed(prompt)
    };
    let args = headless_vendor_args(
        host_main,
        vendor,
        &effective_prompt,
        session_id,
        contained,
        model,
        effort,
    )?;
    let args = match fork_thread {
        Some(thread) => codex_exec_fork_args(args, thread)?,
        None => args,
    };
    os_wrapped_program_and_args(
        host_main,
        &worktree,
        &resolve_agent_program(vendor.program()),
        args,
    )
}

/// Rewrite a fresh `codex [posture] exec [opts] PROMPT` argv into
/// `codex [posture] exec fork [opts] THREAD PROMPT`. Only the subcommand and
/// the positional change; every posture flag is kept as built. The prompt is
/// the final element of every Codex headless argv.
// trace:STORY-2255 | ai:claude
pub(crate) fn codex_exec_fork_args(args: Vec<String>, thread: &str) -> Result<Vec<String>> {
    crate::vendors::fork_thread_args(args, thread)
}

/// How long the `codex exec fork --help` probe may run. A codex that blocks
/// (an update or login prompt, a wedged npm shim) must not hang an advisor
/// tick. trace:STORY-2255 | ai:claude
pub(crate) const CODEX_EXEC_FORK_PROBE_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(10);

/// Whether the installed codex has `exec fork` (codex-cli 0.166+). Probes
/// `codex exec fork --help`, bounded by [`CODEX_EXEC_FORK_PROBE_TIMEOUT`].
/// A spawn failure or non-zero exit is [`advisor::ExecForkSupport::Missing`];
/// a probe that does not answer in time is `TimedOut`.
// trace:STORY-2255 | ai:claude
pub(crate) fn codex_exec_fork_support() -> crate::advisor::ExecForkSupport {
    codex_exec_fork_support_within(CODEX_EXEC_FORK_PROBE_TIMEOUT)
}

pub(crate) fn codex_exec_fork_support_within(
    timeout: std::time::Duration,
) -> crate::advisor::ExecForkSupport {
    use crate::advisor::ExecForkSupport;
    let mut cmd =
        std::process::Command::new(resolve_agent_program(HeadlessVendor::Codex.program()));
    cmd.args(["exec", "fork", "--help"])
        .stdin(std::process::Stdio::null());
    match crate::command_output_with_timeout_detail(cmd, timeout) {
        crate::BoundedCommandOutput::Completed(out)
            if out.status.success()
                && String::from_utf8_lossy(&out.stdout).contains("SESSION_ID") =>
        {
            ExecForkSupport::Supported
        }
        crate::BoundedCommandOutput::TimedOut => ExecForkSupport::TimedOut,
        _ => ExecForkSupport::Missing,
    }
}

/// BUG-044: [`compose_headless_command`], with `resume` rewriting the
/// composed `--session-id <id>` to `--resume <id>` so a resume keeps the
/// launch's posture, model and wrapper. One shared builder, so the BUG-342
/// ratchet sees every headless launch route through it.
///
/// BUG-2386: Codex resumes too, as `codex exec resume <id> <prompt>`; there
/// `session_id` must be the Codex thread id ([`crate::codex_resume`]). Agy has
/// no resume surface and is refused by name.
// trace:BUG-B6B1278890774EF993D2EBC327862470-044 | ai:claude
// trace:BUG-2386 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn compose_headless_command_for(
    host_main: Option<&Path>,
    vendor: HeadlessVendor,
    prompt: &str,
    session_id: &str,
    contained: bool,
    model: Option<&str>,
    effort: Option<&str>,
    resume: bool,
) -> Result<(String, Vec<String>)> {
    let (program, args) = compose_headless_command(
        host_main, vendor, prompt, session_id, contained, model, effort,
    )?;
    if !resume {
        return Ok((program, args));
    }
    match vendor {
        HeadlessVendor::Claude => Ok((program, resume_session_args(args, session_id)?)),
        HeadlessVendor::Codex => Ok((program, codex_resume_exec_args(args, session_id)?)),
        HeadlessVendor::Agy => anyhow::bail!("{AGY_RESUME_REFUSAL}"),
    }
}

/// BUG-2386: agy has no verified resume surface.
pub(crate) const AGY_RESUME_REFUSAL: &str =
    "an agy session cannot be resumed: agy has no resume command. Start a fresh run instead.";

/// STORY-683: spawn (not exec) a headless run of `vendor`'s CLI and wait,
/// returning the exit status. The vendor-neutral generalization of
/// [`spawn_claude_headless`]: it builds the right argv per vendor
/// ([`headless_vendor_args`] — `claude -p …` vs `codex exec …`), applies the
/// opt-in OS-boundary wrapper (`bwrap`, STORY-612) around whichever program, and
/// sets `AIDA_HEADLESS=1` in the env. The Claude path is unchanged from the
/// pre-STORY-683 behavior. trace:STORY-683 | ai:claude
pub fn spawn_vendor_headless(
    vendor: HeadlessVendor,
    prompt: &str,
    session_id: &str,
    log_path: &Path,
    tee_opts: &crate::headless_tee::TeeOptions,
    contained: bool,
) -> Result<std::process::ExitStatus> {
    spawn_vendor_headless_with_seat(
        vendor,
        aida_core::agents_config::AgentSeat::Implementer,
        prompt,
        session_id,
        log_path,
        tee_opts,
        contained,
    )
}

// trace:STORY-1033 | ai:codex
pub fn spawn_vendor_headless_with_seat(
    vendor: HeadlessVendor,
    seat: aida_core::agents_config::AgentSeat,
    prompt: &str,
    session_id: &str,
    log_path: &Path,
    tee_opts: &crate::headless_tee::TeeOptions,
    contained: bool,
) -> Result<std::process::ExitStatus> {
    // TASK-2032: an entry wrapper, called from a host cwd (never after a
    // chdir into a seat); it resolves the host main once for the builders.
    // trace:TASK-2032 | ai:claude
    spawn_vendor_headless_with_seat_granted(
        crate::seat_clone::launcher_main().as_deref(),
        vendor,
        seat,
        prompt,
        session_id,
        log_path,
        tee_opts,
        contained,
    )
    .map(|(status, _)| status)
}

/// BUG-2149: [`spawn_vendor_headless_with_seat`], also returning the id of the
/// child seat grant it issued, so the caller can bind the child's recorded
/// output to exactly this launch.
// trace:BUG-2149 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_vendor_headless_with_seat_granted(
    host_main: Option<&Path>,
    vendor: HeadlessVendor,
    seat: aida_core::agents_config::AgentSeat,
    prompt: &str,
    session_id: &str,
    log_path: &Path,
    tee_opts: &crate::headless_tee::TeeOptions,
    contained: bool,
) -> Result<(std::process::ExitStatus, String)> {
    spawn_vendor_headless_forking(
        host_main, vendor, seat, prompt, session_id, log_path, tee_opts, contained, None, None,
    )
}

/// Fork the live Codex thread `thread` headless (`codex exec fork <thread>
/// <prompt>`) under `seat`, with every guard of a fresh headless launch:
/// containment refusal, posture, grant, env. The live thread is only read;
/// Codex writes the fork as a new session.
// trace:STORY-2255 | ai:claude
pub(crate) fn spawn_codex_headless_fork(
    seat: aida_core::agents_config::AgentSeat,
    prompt: &str,
    thread: &str,
    session_id: &str,
    log_path: &Path,
    tee_opts: &crate::headless_tee::TeeOptions,
    contained: bool,
) -> Result<std::process::ExitStatus> {
    // TASK-2032: an entry wrapper run from a host cwd. trace:TASK-2032 | ai:claude
    spawn_vendor_headless_forking(
        crate::seat_clone::launcher_main().as_deref(),
        HeadlessVendor::Codex,
        seat,
        prompt,
        session_id,
        log_path,
        tee_opts,
        contained,
        Some(thread),
        None,
    )
    .map(|(status, _)| status)
}

#[allow(clippy::too_many_arguments)]
fn spawn_vendor_headless_forking(
    host_main: Option<&Path>,
    vendor: HeadlessVendor,
    seat: aida_core::agents_config::AgentSeat,
    prompt: &str,
    session_id: &str,
    log_path: &Path,
    tee_opts: &crate::headless_tee::TeeOptions,
    contained: bool,
    fork_thread: Option<&str>,
    customize: Option<&dyn Fn(&mut std::process::Command)>,
) -> Result<(std::process::ExitStatus, String)> {
    use std::process::Stdio;
    // trace:BUG-2172 | ai:codex
    let contained = aida_contained_env()? || contained;
    // BUG-2144: refuse before the log file and tee exist. trace:BUG-2144 | ai:claude
    require_vendor_containment(vendor, contained, &format!("{} session", seat.as_str()))?;
    if let Some(dir) = log_path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
    }
    let log = std::fs::File::create(log_path)
        .with_context(|| format!("failed to create headless log {}", log_path.display()))?;
    let tee = crate::headless_tee::start_tee(log_path, tee_opts);
    // STORY-683: surface a non-default vendor so an operator watching a drain
    // knows it is running on Codex, not Claude. The default (Claude) launch
    // stays silent so existing output is unchanged. trace:STORY-683 | ai:claude
    if vendor != HeadlessVendor::Claude {
        eprintln!(
            "{} headless drain phase on vendor `{}` seat `{}`",
            "Vendor:".cyan().bold(),
            vendor.as_str(),
            seat.as_str()
        );
    }
    let tuning = resolve_agent_tuning(&headless_worktree_root(), vendor, seat);
    // TASK-1169 / ADR-22: bound the child's turn-end background-wait ceiling
    // ourselves rather than inheriting whatever the ambient shell carried, so
    // every headless phase behaves identically whoever launched it.
    // trace:TASK-1169 | ai:claude
    let (ceiling_key, ceiling_value) = crate::bg_wait_ceiling_env(Some(&headless_worktree_root()));
    // trace:TASK-2708 | ai:claude — the shared wrapper owns this launch's
    // boundary (Codex `aida`, or Claude once the trusted config selects it).
    let wrapper = contained_wrapper_launch(vendor, contained)?;
    let composition =
        (wrapper && vendor == HeadlessVendor::Claude).then(ClaudeWrapperComposition::enter);
    let (program, mut args) = compose_headless_command_forking(
        host_main,
        vendor,
        prompt,
        session_id,
        contained,
        tuning.model.as_deref(),
        tuning.effort.as_deref(),
        fork_thread,
    )?;
    drop(composition);
    // Each seat gets its own role's write set. trace:TASK-2712 | ai:claude
    rescope_native_codex_for_seat(
        host_main,
        vendor,
        &mut args,
        &headless_worktree_root(),
        seat.as_str(),
    )?;
    // The boundary (the contained mailbox) is set up before any grant exists.
    // trace:TASK-2631 | ai:codex
    let posture = containment_posture(vendor, contained)?;
    // trace:STORY-2326 TASK-2032 | ai:claude
    let ctx = headless_launch_context(host_main, vendor, program, args, posture)?
        .with_wrapper_environment(wrapper);
    let mut cmd = crate::launch::spawn_agent(&ctx)?;
    // TASK-2708: the headless env sits beside its compose_headless_command
    // build (BUG-342); the child grant is issued just after.
    cmd.env("AIDA_HEADLESS", "1")
        // BUG-802: anchor the drive root so `aida review record --pr N` (and
        // any future handshake writer) lands artifacts where the orchestrator
        // polls, no matter where the session wanders (PR checkouts, /tmp).
        .env("AIDA_DRIVE_ROOT", headless_worktree_root())
        // BUG-806: phase agents hold the drive's own binary, not whatever the
        // machine's login shells happen to resolve.
        .env("PATH", drive_path_env().unwrap_or_default())
        .envs(crate::headless_wait_envs(ceiling_key, &ceiling_value));
    // Match the command customization boundary of run_vendor_headless_child.
    // trace:TASK-3225 | ai:codex
    if let Some(customize) = customize {
        customize(&mut cmd);
    }
    // trace:TASK-3225 | ai:codex — refuse before issuing a grant or spawning natively.
    crate::containment::launch::require_store_service_surface(&cmd, wrapper)?;
    // trace:BUG-b6b1278890774ef993d2ebc327862470-045 | ai:claude
    let child_grant = crate::launch::issue_launch_child(
        &headless_worktree_root(),
        seat.as_str(),
        &crate::current_user_id(None),
        // trace:TASK-2708 | ai:claude
        sandboxed_child_grant(vendor, contained)?,
    )?;
    // trace:BUG-2187 | ai:codex
    // trace:STORY-1843 | ai:codex
    let mut handle_guard = child_reservation_guard();
    apply_child_agent_identity(&mut cmd, vendor, seat.as_str());
    // trace:TASK-1955 | ai:claude (TASK-1804)
    apply_headless_harness_version(host_main, &mut cmd, vendor, &headless_worktree_root());
    // BUG-2139: the child holds its own grant; the launcher's purpose stays
    // behind. trace:BUG-2139 STORY-2326 | ai:claude
    crate::launch::assign_seat(
        &mut cmd,
        ctx.kind(),
        crate::launch::SeatDecision::Child {
            role: seat.as_str(),
            grant: &child_grant.id,
        },
    )?;
    if vendor == HeadlessVendor::Codex || wrapper {
        cmd.env("AIDA_USER", crate::current_user_id(None))
            .env("AIDA_PROJECT_ROOT", headless_worktree_root());
    }
    if vendor == HeadlessVendor::Codex && posture != ContainmentPosture::Aida {
        if let Err(error) = crate::vendors::profile::finalize_codex(&mut cmd) {
            revoke_headless_child_grant(&child_grant.id);
            return Err(error);
        }
    }
    // trace:TASK-2707 | ai:codex
    // trace:TASK-2708 | ai:claude
    crate::reviewer_completion::bind_grant(&child_grant.id); // trace:BUG-2546 | ai:codex
    if wrapper {
        let result = (|| -> Result<_> {
            let mut run = crate::containment::launch::PreparedLaunch::prepare(
                &mut cmd,
                &headless_worktree_root(),
                host_main,
            )?
            .spawn(Stdio::from(log))?;
            anyhow::ensure!(
                containment_applied(vendor, true, Some(run.receipt())),
                "missing applied receipt"
            );
            if let Some(guard) = &mut handle_guard {
                guard.disarm();
            }
            // trace:TASK-2708 | ai:claude
            crate::gate::seat_authority::bind_contained_launch_worker(
                &child_grant.id,
                run.receipt(),
            )?;
            run.release()?;
            let status = crate::reviewer_completion::wait(&mut run.child)
                .with_context(|| format!("failed to wait for contained {}", vendor.program()))?;
            run.vendor_exit(status)
        })();
        tee.stop();
        record_headless_session_end(&headless_worktree_root(), session_id);
        if result.is_err() {
            revoke_headless_child_grant(&child_grant.id);
        }
        return result.map(|status| (status, child_grant.id));
    }
    // A watch-launched reviewer is recorded before the wait. trace:TASK-1993 | ai:claude
    let status = reviewer_status_with_handle_guard(cmd.stdout(Stdio::from(log)), &mut handle_guard)
        .with_context(|| format!("failed to spawn {}", vendor.program()))?;
    tee.stop();
    // trace:TASK-2838 | ai:codex
    let status = if vendor == HeadlessVendor::Agy {
        crate::agy_outcome::log_status(log_path, status)
    } else {
        status
    };
    // The child has exited: end its registry row now. Its prompts are its only
    // heartbeats and a `-p` lane gets one, so gc would end it at its start.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-151 | ai:claude
    record_headless_session_end(&headless_worktree_root(), session_id);
    Ok((status, child_grant.id))
}

/// The context of a headless vendor child, which inherits this process's cwd
/// and is contained (when it is) at the drive's worktree root.
// trace:STORY-2326 | ai:claude
pub(crate) fn headless_launch_context(
    host_main: Option<&Path>,
    vendor: HeadlessVendor,
    program: impl Into<std::ffi::OsString>,
    args: Vec<String>,
    posture: ContainmentPosture,
) -> Result<crate::launch::LaunchContext> {
    Ok({
        let req = crate::launch::LaunchRequest {
            vendor: crate::launch::context::launch_vendor(vendor),
            kind: aida_core::launch_policy::LaunchKind::Fresh,
            program: program.into(),
            args,
            tail: Vec::new(),
            posture,
            cwd: headless_worktree_root(),
            set_cwd: false,
            // trace:TASK-2032 | ai:claude
            host_main: host_main.map(Path::to_path_buf),
        };
        // trace:STORY-2538 | ai:codex
        let exec = crate::session::capture_launch_exec(&req.cwd, req.host_main.as_deref())?;
        crate::launch::LaunchContext::resolve(&exec, req)
    }?)
}

/// End the registry row of the headless session `session_id`, which ran from
/// `worktree_root`, at its real exit. Warns, never fails the phase.
// trace:BUG-B6B1278890774EF993D2EBC327862470-151 | ai:claude
pub(crate) fn record_headless_session_end(worktree_root: &Path, session_id: &str) {
    let project_root = crate::main_worktree_root_from(worktree_root);
    if let Err(err) = crate::agent_registry::mark_session_ended(&project_root, session_id) {
        eprintln!(
            "{} failed to mark the headless session's registry entry ended: {err}",
            "warning:".yellow().bold()
        );
    }
}

/// STORY-612: the worktree root a non-resume headless drain runs in — its
/// `.aida-store` sibling and the worktree itself are the rw surfaces the OS
/// wrapper binds. Resolve the project root from cwd, falling back to cwd itself
/// (and `.` if even that fails) so the wrapper never panics on a stray env.
///
/// BUG-1971: the contained WRITE boundary never reads this root, because its
/// cwd fallback could be `$HOME`. It uses [`contained_session_write_roots`],
/// which has no fallback.
/// trace:STORY-612 | ai:claude
fn headless_worktree_root() -> PathBuf {
    // trace:BUG-2001 | ai:antigravity
    #[cfg(test)]
    if let Some(cwd) = contained_cwd_seam::cwd() {
        return crate::find_project_root_from(&cwd).unwrap_or(cwd);
    }
    crate::find_project_root()
        .ok()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// STORY-683: which vendor's headless CLI drives an orchestrator drain phase.
/// The autonomous drain (`burndown` / `queue work --auto-complete --no-human`)
/// used to hardcode `claude -p`; this enum lets the same spawn path launch
/// `codex exec` instead, so a sustained headless drain can run on Codex.
///
/// `Claude` is the default everywhere — selecting `Codex` is an explicit opt-in
/// (via `AIDA_HEADLESS_VENDOR=codex` or `[orchestrator] headless_vendor =
/// "codex"`), so an un-configured drain is byte-identical to the pre-STORY-683
/// behavior. Prior art for the `codex exec` adapter is `compete.rs::vendor_adapter`.
/// trace:STORY-683 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadlessVendor {
    /// `claude -p …` — the default. The SPIKE-7 mandatory flag set.
    Claude,
    /// `codex exec …` — the Codex headless CLI. trace:STORY-683
    Codex,
    /// `agy … -p <prompt>` — the Antigravity (AGY) headless CLI. The `agy`
    /// runtime's non-interactive `-p`/`--print` mode is the AGY analogue of
    /// `claude -p` / `codex exec` (verified in SPIKE-27's agy-architecture
    /// surface note). BUG-1686: agy's `-p` TAKES the prompt as its value, so it
    /// comes last, after every option flag.
    // trace:TASK-1048 BUG-1686 | ai:claude
    Agy,
}

// trace:STORY-2539 | ai:codex
impl HeadlessVendor {
    pub(crate) fn launch_vendor(self) -> aida_core::launch_policy::LaunchVendor {
        use aida_core::launch_policy::LaunchVendor;
        match self {
            Self::Claude => LaunchVendor::Claude,
            Self::Codex => LaunchVendor::Codex,
            Self::Agy => LaunchVendor::Agy,
        }
    }

    pub(crate) fn profile(self) -> &'static aida_core::vendors::VendorProfile {
        aida_core::vendors::profile(self.launch_vendor())
    }

    pub fn program(self) -> &'static str {
        self.profile().executable
    }

    /// Preserve session/config spelling, including `agy` versus `antigravity`.
    pub fn as_str(self) -> &'static str {
        self.profile().session_name
    }

    pub fn parse(raw: &str) -> Option<Self> {
        use aida_core::launch_policy::LaunchVendor;
        Some(match LaunchVendor::parse(raw).ok()? {
            LaunchVendor::Claude => Self::Claude,
            LaunchVendor::Codex => Self::Codex,
            LaunchVendor::Agy => Self::Agy,
        })
    }
}

/// TASK-1116: per-invocation top-precedence override for the headless
/// implementer vendor, set from the `--vendor`/`--agent` flag on a drive
/// command (`aida zen`, `aida queue work`, `aida burndown run`) before the
/// drain spawns. Wins over `AIDA_HEADLESS_VENDOR` and the config knobs in
/// [`resolve_headless_vendor`]. Encoding: `0` = unset (an un-flagged run is
/// byte-identical to the pre-TASK-1116 resolution), `1` = Claude, `2` = Codex,
/// `3` = Agy.
///
/// Process-global rather than a threaded parameter because the flag is parsed
/// at the top-level dispatch while the in-process spawn paths
/// ([`spawn_claude_headless`] / [`exec_claude_headless`]) resolve deep in the
/// call graph; a child-*subprocess* implementer (the orchestrator spawns
/// `aida queue work <spec> --no-human`) inherits the same choice via the
/// `AIDA_HEADLESS_VENDOR` env that [`install_headless_vendor_override`] also
/// exports.
// trace:TASK-1116 | ai:claude
static HEADLESS_VENDOR_OVERRIDE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// TASK-1116: set (or clear, with `None`) the per-invocation headless-vendor
/// override tier consulted first by [`resolve_headless_vendor`].
// trace:TASK-1116 | ai:claude
pub(crate) fn set_headless_vendor_override(vendor: Option<HeadlessVendor>) {
    let code = match vendor {
        None => 0,
        Some(HeadlessVendor::Claude) => 1,
        Some(HeadlessVendor::Codex) => 2,
        Some(HeadlessVendor::Agy) => 3,
    };
    HEADLESS_VENDOR_OVERRIDE.store(code, std::sync::atomic::Ordering::SeqCst);
}

/// TASK-1116: read the per-invocation override tier.
// trace:TASK-1116 | ai:claude
fn headless_vendor_override() -> Option<HeadlessVendor> {
    match HEADLESS_VENDOR_OVERRIDE.load(std::sync::atomic::Ordering::SeqCst) {
        1 => Some(HeadlessVendor::Claude),
        2 => Some(HeadlessVendor::Codex),
        3 => Some(HeadlessVendor::Agy),
        _ => None,
    }
}

/// TASK-1116: install a `--vendor`/`--agent` flag value as the top-precedence
/// headless-vendor override for this invocation. Recognized tokens
/// (`claude` / `codex` / `agy` [alias `antigravity`]) set BOTH:
///   - the in-process override tier ([`set_headless_vendor_override`]) — so a
///     spawn path that resolves in THIS process (`aida queue work <spec>
///     --no-human`) routes to it, and
///   - the `AIDA_HEADLESS_VENDOR` env — so a child-subprocess drain (the
///     `--auto-complete` orchestrator's implementer child, or `aida zen`'s
///     self-invoked `aida queue work --auto-complete`) inherits it.
/// Returns the parsed vendor, or `None` for an unrecognized token so the caller
/// can choose to error (the new `zen` / `burndown` flags) or fall through (the
/// pre-existing `queue work --vendor`, whose interactive host path tolerates
/// other values).
// trace:TASK-1116 | ai:claude
pub(crate) fn install_headless_vendor_override(raw: &str) -> Option<HeadlessVendor> {
    let vendor = HeadlessVendor::parse(raw)?;
    set_headless_vendor_override(Some(vendor));
    std::env::set_var("AIDA_HEADLESS_VENDOR", vendor.as_str());
    Some(vendor)
}

/// STORY-683: resolve the vendor a headless drain spawn should use. Resolution
/// order, highest precedence first:
///   0. TASK-1116 per-invocation `--vendor`/`--agent` flag override (in-process
///      tier) — wins over everything below so a drive command can pick the
///      implementer vendor for one run.
///   1. `AIDA_HEADLESS_VENDOR` env (per-host / per-invocation override) — mirrors
///      the `AIDA_OS_WRAP` precedence convention; an unrecognized value is ignored.
///   2. `[orchestrator] headless_vendor` in the project config.
///   3. `[agents] vendor` in agents.toml (project over global) — the STORY-761
///      set-once default-vendor knob for codex-first machines.
///   4. `Claude` (default) — so an un-configured drain is unchanged.
/// `worktree_root` roots the config read.
// trace:STORY-683 STORY-761 TASK-1116 | ai:claude
pub(crate) fn resolve_headless_vendor(worktree_root: &Path) -> HeadlessVendor {
    // TASK-1116: the per-invocation flag override wins over env + knobs.
    if let Some(v) = headless_vendor_override() {
        return v;
    }
    if let Some(raw) = std::env::var("AIDA_HEADLESS_VENDOR").ok() {
        if let Some(v) = HeadlessVendor::parse(&raw) {
            return v;
        }
    }
    let cfg = crate::read_project_config_value(worktree_root);
    crate::config_lookup(cfg.as_ref(), "orchestrator", "headless_vendor")
        .and_then(|v| v.as_str())
        .and_then(HeadlessVendor::parse)
        .or_else(|| {
            aida_core::agents_config::resolve_default_vendor(worktree_root)
                .as_deref()
                .and_then(HeadlessVendor::parse)
        })
        .unwrap_or(HeadlessVendor::Claude)
}

/// STORY-1003: resolve the opaque model string for a vendor-backed launch.
///
/// The one-shot env tier is how `aida queue work --model` propagates into
/// orchestrator phase children without teaching the orchestrator model names.
/// Empty values are unset; AIDA never validates the model token.
// trace:STORY-1003 | ai:codex
pub(crate) fn resolve_vendor_model(worktree_root: &Path, vendor: HeadlessVendor) -> Option<String> {
    std::env::var("AIDA_AGENT_MODEL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| aida_core::agents_config::resolve_vendor_model(worktree_root, vendor.as_str()))
}

/// Resolve model/effort for a vendor-backed launch seat.
///
/// `AIDA_AGENT_MODEL` remains the one-shot model override for legacy callers;
/// `AIDA_AGENT_EFFORT` is the effort twin. Config fills any unset field from
/// `[agents.<vendor>]` / `[agents.<vendor>.seats]`.
// trace:STORY-1033 | ai:codex
pub(crate) fn resolve_agent_tuning(
    worktree_root: &Path,
    vendor: HeadlessVendor,
    seat: aida_core::agents_config::AgentSeat,
) -> aida_core::agents_config::ResolvedAgentTuning {
    let mut tuning =
        aida_core::agents_config::resolve_agent_tuning(worktree_root, vendor.as_str(), seat);
    if let Some(model) = std::env::var("AIDA_AGENT_MODEL")
        .ok()
        .filter(|s| !s.trim().is_empty())
    {
        tuning.model = Some(model);
    }
    if let Some(effort) = std::env::var("AIDA_AGENT_EFFORT")
        .ok()
        .filter(|s| !s.trim().is_empty())
    {
        tuning.effort = Some(effort);
    }
    tuning
}

fn agent_selection_allows_vendor(
    selection: crate::init_cmd::AgentSelection,
    vendor: HeadlessVendor,
) -> bool {
    match vendor {
        HeadlessVendor::Claude => selection.claude,
        HeadlessVendor::Codex => selection.codex,
        HeadlessVendor::Agy => selection.antigravity,
    }
}

pub(crate) fn enabled_headless_vendors(
    selection: crate::init_cmd::AgentSelection,
) -> Vec<HeadlessVendor> {
    let mut vendors = Vec::new();
    if selection.claude {
        vendors.push(HeadlessVendor::Claude);
    }
    if selection.codex {
        vendors.push(HeadlessVendor::Codex);
    }
    if selection.antigravity {
        vendors.push(HeadlessVendor::Agy);
    }
    vendors
}

fn explicit_headless_vendor_selected(worktree_root: &Path) -> bool {
    if headless_vendor_override().is_some() {
        return true;
    }
    if std::env::var("AIDA_HEADLESS_VENDOR")
        .ok()
        .and_then(|raw| HeadlessVendor::parse(&raw))
        .is_some()
    {
        return true;
    }
    let cfg = crate::read_project_config_value(worktree_root);
    if crate::config_lookup(cfg.as_ref(), "orchestrator", "headless_vendor")
        .and_then(|v| v.as_str())
        .and_then(HeadlessVendor::parse)
        .is_some()
    {
        return true;
    }
    aida_core::agents_config::resolve_default_vendor(worktree_root)
        .as_deref()
        .and_then(HeadlessVendor::parse)
        .is_some()
}

/// BUG-898: queue-work launch preflight. The launch vendor must be resolved
/// and checked against the persisted `[agents] enabled` profile before
/// `session_start` creates a worktree or writes a lease.
// trace:BUG-898 | ai:codex
/// BUG-2386: the gate a resume passes instead of the configured-vendor check.
/// A resume continues the session on the vendor that RECORDED it, so that
/// vendor must be resumable and enabled by `[agents] enabled`; the configured
/// launch vendor does not matter.
// trace:BUG-2386 | ai:claude
pub(crate) fn require_resume_vendor(worktree_root: &Path, recorded: HeadlessVendor) -> Result<()> {
    if !vendor_can_resume(recorded) {
        anyhow::bail!("{AGY_RESUME_REFUSAL}");
    }
    if let Some(selection) = crate::init_cmd::read_enabled_agent_selection(worktree_root) {
        if !agent_selection_allows_vendor(selection, recorded) {
            anyhow::bail!(
                "this session was recorded by `{}`, which `[agents] enabled` disables; refusing \
                 to resume it on another vendor. Recovery: enable `{}`, or start a fresh run.",
                recorded.as_str(),
                recorded.as_str()
            );
        }
    }
    Ok(())
}

pub(crate) fn resolve_enabled_headless_vendor(worktree_root: &Path) -> Result<HeadlessVendor> {
    let resolved = resolve_headless_vendor(worktree_root);
    let Some(selection) = crate::init_cmd::read_enabled_agent_selection(worktree_root) else {
        return Ok(resolved);
    };
    if agent_selection_allows_vendor(selection, resolved) {
        return Ok(resolved);
    }

    let enabled = enabled_headless_vendors(selection);
    if enabled.is_empty() {
        anyhow::bail!(
            "no agent launch profiles are enabled by `[agents] enabled`; refusing before creating \
             a worktree or lease. Recovery: enable a profile such as `codex`, or re-run with \
             `--no-launch` to prepare state without launching an agent."
        );
    }

    if enabled.len() == 1 && !explicit_headless_vendor_selected(worktree_root) {
        return Ok(enabled[0]);
    }

    let enabled_names = enabled
        .iter()
        .map(|v| v.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    anyhow::bail!(
        "resolved launch vendor `{}` is disabled by `[agents] enabled` (enabled: {}). \
         Refusing before creating a worktree or lease. Recovery: re-run with an enabled explicit \
         vendor, for example `--vendor codex`, or use `--no-launch`.",
        resolved.as_str(),
        enabled_names
    );
}

/// BUG-1607: verify the resolved launch vendor's CLI is actually reachable —
/// BEFORE a caller mints a lease or worktree. [`resolve_enabled_headless_vendor`]
/// only checks the `[agents] enabled` config; it can resolve to a vendor that
/// is enabled but never installed. The observed failure: a codex-only project
/// correctly resolved `vendor: codex`, created the reviewer lease + worktree,
/// then the launch itself hardcoded `claude` and failed with a raw ENOENT —
/// leaving a live lease/worktree behind with no process. Every launch site
/// that mutates state before spawning MUST call this first, in addition to
/// (not instead of) `resolve_enabled_headless_vendor`, and propagate the
/// error unchanged.
///
/// Routes the binary name through [`resolve_agent_program`] first, so tests
/// can inject a fake reachable/unreachable "binary" via `AIDA_AGENT_CMD`
/// without spawning a real vendor CLI — the same seam the headless spawn
/// paths already use.
// trace:BUG-1607 | ai:claude
pub(crate) fn preflight_vendor_binary(vendor: HeadlessVendor) -> Result<()> {
    let program = resolve_agent_program(vendor.program());
    let reachable = if program.contains(std::path::MAIN_SEPARATOR) {
        Path::new(&program).is_file()
    } else {
        which_on_path(&program).is_some()
    };
    if reachable {
        return Ok(());
    }
    anyhow::bail!(
        "the resolved launch vendor `{}` (`{program}`) is not on PATH — refusing before \
         creating a lease or worktree. Recovery: install `{program}`, choose an installed \
         vendor with `--vendor <name>`, or point `[agents] vendor` / `[agents] enabled` in \
         agents.toml at a profile that is actually installed.",
        vendor.as_str()
    );
}

/// BUG-1607: preflight a launch of `vendor` — vendor SUPPORT (for an
/// `interactive` launch, only Agy is refused: the AGY dispatch policy is
/// draft-for-review-only, mechanical/bounded work, with no interactive
/// keystone dialog) AND binary reachability, checked in that order, both
/// BEFORE any caller mints a lease or worktree.
///
/// This closes an ordering gap that had the exact BUG-1607 shape: before
/// this function existed, an interactive Agy launch passed
/// `preflight_vendor_binary` cleanly whenever the `agy` binary happened to
/// be reachable, so `session_start` minted the lease + worktree, and only
/// THEN did [`interactive_reviewer_launch_plan`] (or the equivalent
/// implementer-side Agy check) refuse — leaving an orphaned lease/worktree
/// behind with a refusal instead of a process, same as the original bug.
/// Folding the support check into the ONE preflight every launch site
/// already calls before mutating state closes it for every reviewer AND
/// implementer entry point at once.
///
/// AGY headless admission also checks the pending all-tool evidence before
/// callers create state; the historical bypass argv cannot launch directly.
// trace:BUG-1607 | ai:claude
pub(crate) fn preflight_launch_vendor(vendor: HeadlessVendor, interactive: bool) -> Result<()> {
    if interactive && vendor == HeadlessVendor::Agy {
        anyhow::bail!(
            "interactive launch does not support vendor `agy` yet. Recovery: re-run with \
             `--no-human` for a headless AGY launch, choose `--vendor claude` or `--vendor \
             codex`, or use `--no-launch`."
        );
    }
    // trace:TASK-3048 | ai:codex
    if vendor == HeadlessVendor::Agy {
        crate::launch::agy::AGY.require_conformance()?;
    }
    preflight_vendor_binary(vendor)
}

/// TASK-1162: resolve the session vendor for a launch rooted at the current
/// project — the same precedence stack as a headless drain spawn (flag
/// override → `AIDA_HEADLESS_VENDOR` → `[orchestrator] headless_vendor` →
/// `[agents] vendor` → Claude), exposed for callers outside this module
/// (the guided-mode dispatch).
// trace:TASK-1162 | ai:claude
pub(crate) fn resolve_session_vendor() -> HeadlessVendor {
    resolve_headless_vendor(&headless_worktree_root())
}

/// TASK-1162: what `aida do <spec>` (mode = guided) launches for a session
/// vendor — the program to spawn in the spec worktree and the initial prompt
/// to seed it with.
// trace:TASK-1162 | ai:claude
#[derive(Debug)]
pub(crate) struct GuidedSessionLaunch {
    /// The PATH binary to spawn (`claude` / `codex`).
    pub(crate) program: &'static str,
    /// The initial prompt: Claude gets the bare slash invocation (expanded
    /// natively from `.claude/skills/`); Codex gets the adapted prompt body.
    pub(crate) prompt: String,
}

/// TASK-1162: guided-mode vendor parity. Claude Code expands
/// `/aida-guided-implement` natively, so the Claude arm keeps the bare slash
/// invocation. Codex never reads `.claude/`, so its arm renders the
/// codex-adapted guided prompt (the BUG-731 conversion: plain-text numbered
/// forks instead of AskUserQuestion, ADRs recorded via the CLI) with the spec
/// substituted in. AGY has NO guided equivalent by dispatch policy —
/// draft-for-review-only, mechanical/bounded work — so its arm refuses with
/// the policy reason instead of launching an interactive keystone dialog.
/// Pure — unit-tested without spawning.
// trace:TASK-1162 | ai:claude
pub(crate) fn guided_session_launch(
    vendor: HeadlessVendor,
    spec: &str,
) -> Result<GuidedSessionLaunch> {
    match vendor {
        HeadlessVendor::Claude => Ok(GuidedSessionLaunch {
            program: "claude",
            prompt: format!("/aida-guided-implement {spec}"),
        }),
        HeadlessVendor::Codex => {
            let prompt = aida_core::scaffolding::codex_prompts::render_codex_command_prompt(
                "aida-guided-implement",
                spec,
            )
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "the codex-adapted guided prompt is missing from the embedded command set — \
                     this is a build defect, not a configuration problem"
                )
            })?;
            Ok(GuidedSessionLaunch {
                program: "codex",
                prompt,
            })
        }
        HeadlessVendor::Agy => anyhow::bail!(
            "guided mode does not run on agy: the AGY dispatch policy fences agy to \
             draft-for-review, mechanical/bounded work, and a guided session is an interactive \
             keystone decision dialog. Re-run with the session vendor set to claude or codex \
             (AIDA_HEADLESS_VENDOR=claude|codex, or `[agents] vendor` in agents.toml)."
        ),
    }
}

/// STORY-683: build the argv (after the program name) for a headless launch of
/// the given vendor. `Claude` reuses the SPIKE-7 mandatory flag set
/// ([`claude_headless_args_with_posture`]); `Codex` builds the `codex exec`
/// argv (prior art: `compete.rs::vendor_adapter`), with the prompt as the final
/// positional. Pure — the arms are unit-tested without spawning. The `contained`
/// posture selects Claude containment or an explicitly opted-in Codex native sandbox.
/// Uncontained Codex and Agy retain their historical bypass arguments.
// trace:TASK-2631 | ai:codex
// trace:STORY-683 TASK-1048 | ai:claude
// trace:BUG-2006 | ai:antigravity
// trace:STORY-2539 | ai:codex
pub fn headless_vendor_args(
    host_main: Option<&Path>,
    vendor: HeadlessVendor,
    prompt: &str,
    session_id: &str,
    contained: bool,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<Vec<String>> {
    crate::vendors::headless::headless_vendor_args(
        host_main, vendor, prompt, session_id, contained, model, effort,
    )
}

/// BUG-2133: [`headless_vendor_args`] for a drain child whose working
/// directory is `contained_in` rather than the drain process's cwd. `Some(dir)`
/// requests the contained posture with the write boundary scoped to `dir`
/// (which must itself be a git root, else no write roots: fail closed); `None`
/// is uncontained. Callers run [`require_child_containment`] first, so a
/// vendor that cannot apply containment never reaches this with `Some`.
// trace:BUG-2133 | ai:claude
// trace:STORY-2539 | ai:codex
pub(crate) fn headless_vendor_args_contained_in(
    host_main: Option<&Path>,
    vendor: HeadlessVendor,
    prompt: &str,
    session_id: &str,
    contained_in: Option<&Path>,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<Vec<String>> {
    crate::vendors::headless::headless_vendor_args_contained_in(
        host_main,
        vendor,
        prompt,
        session_id,
        contained_in,
        model,
        effort,
    )
}

/// TASK-1048: the `agy` argv (after the `agy` program name) for a one-shot
/// headless run. Mirrors the Codex arm's shape: AGY's non-interactive `-p`/
/// `--print` mode is the AGY analogue of `claude -p` / `codex exec` — it runs a
/// single prompt and exits (SPIKE-27's agy-architecture surface note). Approvals
/// are skipped (`--dangerously-skip-permissions`, the same knob the supervised
/// `aida agent new antigravity --bypass-sandbox` launch uses) so the run is
/// unattended. Like Codex, AGY has no caller-minted `--session-id` /
/// `stream-json` machinery threaded here, so the arm does not carry `session_id`.
///
/// BUG-1686: unlike Claude's boolean `-p`, Antigravity's `-p`/`--print` TAKES A
/// VALUE, so the prompt is that flag's value and `-p <prompt>` must come LAST,
/// after every option flag. Pure — unit-tested without spawning.
// trace:TASK-1048 BUG-1686 | ai:claude
// trace:TASK-1581 | ai:antigravity — test-only default-argument wrapper; every
// production launch builds its argv through `headless_vendor_args`.
#[cfg(test)]
pub fn agy_headless_args(prompt: &str) -> Vec<String> {
    agy_headless_args_with_effort(prompt, None)
}

// trace:STORY-1033 | ai:codex
// BUG-1686: argv ORDER is load-bearing here. Antigravity CLI 1.2.12 parses
// Go-style flags and `-p` / `--print` / `--prompt` is a VALUE-taking flag
// ("Run a single prompt non-interactively and print the response"), not the
// boolean `-p` claude has. The pre-BUG-1686 order emitted
// `-p --dangerously-skip-permissions [--effort E] <prompt>`, so agy took the
// literal string "--dangerously-skip-permissions" as the prompt, never saw the
// permission flag as a flag, and rejected the launch — `aida queue work <SPEC>
// --vendor agy --no-human=both --strict` left a lease and worktree behind with
// no implementer process. Go's flag parser also stops at the first non-flag
// argument, so a trailing positional prompt is not an option either: EVERY
// option flag precedes `-p`, and the prompt is `-p`'s value at the tail.
// The permission posture is unchanged — the same single flag, moved, not weakened.
// trace:BUG-1686 | ai:claude
// trace:STORY-2539 | ai:codex
pub fn agy_headless_args_with_effort(prompt: &str, effort: Option<&str>) -> Vec<String> {
    crate::vendors::headless::agy_headless_args_with_effort(prompt, effort)
}

/// STORY-683: the `codex exec` argv (after the `codex` program name) for a
/// one-shot headless run. Mirrors the working `compete.rs` adapter:
/// `exec --json --dangerously-bypass-approvals-and-sandbox <prompt>`. Codex's
/// headless `exec` is single-shot and exits on its own (the orchestrator
/// analogue of claude's `-p`); approvals are bypassed so the run is unattended.
/// The prompt is the final positional. Unlike claude there is no `--session-id`
/// (codex has no matching caller-minted resumable session model), so the codex
/// arm does not thread `session_id`.
///
/// `--json` is load-bearing for drains: without it, `codex exec` writes only the
/// final message at process exit, leaving `.aida/headless-logs` at 0 bytes while
/// Codex is actively working. Streaming JSONL makes the existing watchdog log
/// mtime/length signal live for Codex phases.
// trace:STORY-683 BUG-909 | ai:codex
// trace:TASK-1581 | ai:antigravity — test-only default-argument wrapper; every
// production launch builds its argv through `headless_vendor_args`.
#[cfg(test)]
pub fn codex_headless_args(prompt: &str) -> Vec<String> {
    codex_headless_args_with_model(prompt, None)
}

// trace:STORY-1003 BUG-909 | ai:codex
pub fn codex_headless_args_with_model(prompt: &str, model: Option<&str>) -> Vec<String> {
    codex_headless_args_with_model_and_effort(prompt, model, None)
}

// trace:STORY-1033 | ai:codex
// trace:STORY-2539 | ai:codex
pub fn codex_headless_args_with_model_and_effort(
    prompt: &str,
    model: Option<&str>,
    effort: Option<&str>,
) -> Vec<String> {
    crate::vendors::headless::codex_headless_args_with_model_and_effort(prompt, model, effort)
}

/// BUG-2386: a composed `codex … exec <flags> <prompt>` argv rewritten to
/// `codex … exec resume <flags> <thread> <prompt>`, every flag kept, so a
/// resume runs with the posture, model and wrapper of the launch it continues
/// (the Codex half of [`resume_session_args`]).
// trace:BUG-2386 | ai:claude
pub(crate) fn codex_resume_exec_args(
    mut args: Vec<String>,
    thread_id: &str,
) -> Result<Vec<String>> {
    anyhow::ensure!(
        crate::codex_resume::valid_thread_id(thread_id),
        "refusing to resume `{thread_id}`: not a Codex thread id"
    );
    let exec = args
        .iter()
        .position(|a| a == "exec")
        .ok_or_else(|| anyhow::anyhow!("no `codex exec` launch to resume"))?;
    anyhow::ensure!(
        args.len() > exec + 1,
        "the `codex exec` launch to resume has no prompt"
    );
    let prompt_at = args.len() - 1;
    args.insert(prompt_at, thread_id.to_string());
    args.insert(exec + 1, "resume".to_string());
    Ok(args)
}

/// BUG-2386: the argv (after `codex`) of an interactive `codex resume
/// <thread>`. A native-contained resume takes Codex's own sandbox flags and
/// never the bypass; otherwise the uniform `[agents] bypass` posture maps to
/// Codex's bypass flag, as a fresh interactive launch does (BUG-743).
/// `read_only` (a plan lane, BUG-2394) forces Codex's read-only sandbox over
/// either posture, exactly as a fresh plan-only launch does.
/// `native` is the resumed lane (its own tree and role) when the resume is
/// native-contained: its write set comes from that lane, never this
/// process's cwd (TASK-2712 F8).
// trace:BUG-2386 trace:TASK-2712 | ai:claude
pub(crate) fn codex_interactive_resume_args(
    thread_id: &str,
    native: Option<CodexLane<'_>>,
    bypass: bool,
    read_only: bool,
) -> Result<Vec<String>> {
    anyhow::ensure!(
        crate::codex_resume::valid_thread_id(thread_id),
        "refusing to resume `{thread_id}`: not a Codex thread id"
    );
    let mut args = if let Some(lane) = native {
        // trace:TASK-2032 | ai:claude
        crate::codex_native_sandbox::codex_native_sandbox_flags_in(
            lane.host_main,
            lane.tree,
            lane.role,
        )?
    } else if bypass {
        vec!["--dangerously-bypass-approvals-and-sandbox".to_string()]
    } else {
        Vec::new()
    };
    args.extend(["resume".to_string(), thread_id.to_string()]);
    // trace:BUG-2394 | ai:claude — a plan lane never writes, resumed or not.
    Ok(if read_only {
        codex_read_only_args(args)
    } else {
        args
    })
}

// trace:TASK-1581 | ai:antigravity — test-only default-argument wrapper; every
// production launch builds its argv through `headless_vendor_args`.
#[cfg(test)]
pub fn claude_headless_args(prompt: &str, session_id: &str) -> Vec<String> {
    claude_headless_args_with_posture(None, prompt, session_id, false).unwrap()
}

pub fn claude_headless_args_with_posture(
    host_main: Option<&Path>,
    prompt: &str,
    session_id: &str,
    contained: bool,
) -> Result<Vec<String>> {
    claude_headless_args_with_posture_and_model(host_main, prompt, session_id, contained, None)
}

// trace:STORY-1003 | ai:codex
pub fn claude_headless_args_with_posture_and_model(
    host_main: Option<&Path>,
    prompt: &str,
    session_id: &str,
    contained: bool,
    model: Option<&str>,
) -> Result<Vec<String>> {
    claude_headless_args_with_posture_model_and_effort(
        host_main, prompt, session_id, contained, model, None,
    )
}

// trace:STORY-1033 | ai:codex
pub fn claude_headless_args_with_posture_model_and_effort(
    host_main: Option<&Path>,
    prompt: &str,
    session_id: &str,
    contained: bool,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<Vec<String>> {
    let roots = contained.then(contained_session_write_roots);
    claude_headless_args_for_roots(
        host_main,
        prompt,
        session_id,
        roots.as_deref(),
        model,
        effort,
    )
}

/// BUG-2133: the `claude -p` argv with the contained write boundary given
/// explicitly. `Some(roots)` is the contained posture (`dontAsk` + the
/// `--settings` boundary over exactly `roots`; empty roots deny every edit);
/// `None` is the uncontained `bypassPermissions` posture. A drain child that
/// runs in a different directory from the drain process (the CI-fix child in
/// the implementer worktree) needs this — the process-cwd roots
/// [`claude_headless_args_with_posture_model_and_effort`] uses would scope it
/// to the wrong tree.
// trace:BUG-2133 | ai:claude
// trace:STORY-2539 | ai:codex
pub(crate) fn claude_headless_args_for_roots(
    host_main: Option<&Path>,
    prompt: &str,
    session_id: &str,
    contained_roots: Option<&[PathBuf]>,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<Vec<String>> {
    crate::vendors::headless::claude_headless_args_for_roots(
        host_main,
        prompt,
        session_id,
        contained_roots,
        model,
        effort,
    )
}

/// BUG-050: THE headless `claude -p` posture lives in `aida-core` (the AI
/// client takes it too); every headless claude argv builder takes its flags
/// from [`headless_claude_posture`] or [`headless_claude_posture_for`] (a
/// ratchet test in this file enforces it across both crates).
// trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
pub(crate) use aida_core::ai::headless_posture::{
    headless_claude_posture, DISABLE_BACKGROUND_TASKS_ENV,
};

/// [`headless_claude_posture`] for a launch that is `contained` in the
/// process cwd's boundary ([`claude_contained_flags`]), or not.
// trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
pub(crate) fn headless_claude_posture_for(
    host_main: Option<&Path>,
    contained: bool,
) -> Result<Vec<String>> {
    let contained_flags = if contained {
        claude_contained_flags(host_main)?
    } else {
        Vec::new()
    };
    headless_claude_posture(contained_flags)
}

/// Test check: `args` carries the [`headless_claude_posture`]: exactly one
/// `--settings` whose `env` turns background tasks off and whose
/// `permissions.deny` names `Monitor` (with the sandbox on when `contained`),
/// and `--disallowed-tools AskUserQuestion Monitor` ended by a flag or the
/// end of the argv.
// trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
#[cfg(test)]
pub(crate) fn assert_headless_posture(args: &[String], contained: bool, label: &str) {
    let settings: Vec<&String> = args
        .windows(2)
        .filter(|w| w[0] == "--settings")
        .map(|w| &w[1])
        .collect();
    assert_eq!(settings.len(), 1, "[{label}] one --settings: {args:?}");
    let settings: serde_json::Value = serde_json::from_str(settings[0]).unwrap();
    assert_eq!(
        settings["env"][DISABLE_BACKGROUND_TASKS_ENV], "1",
        "[{label}] {settings}"
    );
    assert!(
        settings["permissions"]["deny"]
            .as_array()
            .is_some_and(|d| d.iter().any(|r| r == "Monitor")),
        "[{label}] Monitor denied: {settings}"
    );
    assert_eq!(
        settings["sandbox"]["enabled"] == true,
        contained,
        "[{label}] {settings}"
    );
    let at = args
        .windows(3)
        .position(|w| w == ["--disallowed-tools", "AskUserQuestion", "Monitor"])
        .unwrap_or_else(|| panic!("[{label}] disallowed tools: {args:?}"));
    assert!(
        args.get(at + 3).is_none_or(|next| next.starts_with('-')),
        "[{label}] the tool list must be ended by a flag, not the prompt: {args:?}"
    );
}

/// STORY-263: replace this process with a headless `claude -p` run — the
/// launch path behind `aida queue work --no-human` (the `--auto-complete`
/// orchestrator's reviewer phase). Claude's stream-json stdout is redirected
/// to `log_path` so the orchestrator's own stdout stays clean (it carries
/// `--json` phase events) and TASK-298's watchdog has a file to tail; stderr
/// stays inherited so Claude errors still surface. trace:STORY-263 | ai:claude
///
/// Sets `AIDA_HEADLESS=1` in the launched environment so skills can tell
/// they are running unattended — the `/aida-review` skill keys its
/// finding-filing step on it, since there is no human to triage the
/// reviewer's findings. trace:STORY-278 | ai:claude
///
/// TASK-307: the headless `claude -p` reviewer/implementer is now hosted by
/// the parent (spawn + wait + exit) instead of `exec`-replacing it, so a
/// background tee thread can tail `log_path` and surface high-signal events
/// to the operator's terminal during the run. The semantics the caller sees
/// are unchanged — the function never returns on success; the process exits
/// with the child's exit code. The pre-TASK-307 `exec()` behaviour is gone
/// because the parent must stay alive to host the tee — including for the
/// `--no-tee-headless` path, where the tee still runs so failure events
/// (`is_error`, `permission_denials`) can never hide. trace:TASK-307 | ai:claude
/// BUG-898: run the already-preflighted queue-work headless vendor. Callers
/// that need to validate `[agents] enabled` before mutating state resolve the
/// vendor first and pass it here, avoiding a second late resolver that could
/// silently fall back to a disabled default.
// trace:BUG-898 | ai:codex
// trace:BUG-2006 | ai:antigravity
// trace:BUG-2139 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_exec_vendor_headless_command(
    host_main: Option<&Path>,
    vendor: HeadlessVendor,
    prompt: &str,
    session_id: &str,
    contained: bool,
    model: Option<&str>,
    effort: Option<&str>,
    seat: &str,
    grant_env: &str,
) -> Result<std::process::Command> {
    build_exec_vendor_headless_command_for(
        host_main, vendor, prompt, session_id, contained, model, effort, seat, grant_env, false,
    )
}

/// [`build_exec_vendor_headless_command`]; `resume` continues `session_id`
/// (Claude) instead of starting it.
// trace:BUG-B6B1278890774EF993D2EBC327862470-044 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_exec_vendor_headless_command_for(
    host_main: Option<&Path>,
    vendor: HeadlessVendor,
    prompt: &str,
    session_id: &str,
    contained: bool,
    model: Option<&str>,
    effort: Option<&str>,
    seat: &str,
    grant_env: &str,
    resume: bool,
) -> Result<std::process::Command> {
    build_coordinator_headless_command_inner(
        host_main, vendor, prompt, session_id, contained, model, effort, seat, grant_env, resume,
        None, true,
    )
}

/// Launch coordinators through the same vendor, identity and containment adapter
/// as queue phases, retaining their permission override and output mode.
/// The caller resolves `vendor` once and passes the same value to every message
/// and to the launch, so what is printed is what runs.
// trace:BUG-2389 | ai:codex
// trace:BUG-2389 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_coordinator_headless_command(
    host_main: Option<&Path>,
    vendor: HeadlessVendor,
    prompt: &str,
    mode: &str,
    seat: aida_core::agents_config::AgentSeat,
    grant_env: &str,
    contained: bool,
    stream: bool,
) -> Result<std::process::Command> {
    let root = headless_worktree_root();
    let tuning = resolve_agent_tuning(&root, vendor, seat);
    let prompt =
        crate::intake::materialize_skill_invocation(prompt, vendor == HeadlessVendor::Claude);
    // Only this coordinator entry point owns the attested spawn below. The
    // exec/resume builder must retain its existing per-surface refusal.
    // trace:TASK-3219 | ai:codex
    let wrapper = vendor == HeadlessVendor::Claude
        && contained_wrapper_launch(vendor, aida_contained_env()? || contained)?;
    let _composition = wrapper.then(ClaudeWrapperComposition::enter);
    let mut command = build_coordinator_headless_command_inner(
        host_main,
        vendor,
        &prompt,
        &uuid::Uuid::now_v7().to_string(),
        contained,
        tuning.model.as_deref(),
        tuning.effort.as_deref(),
        seat.as_str(),
        grant_env,
        false,
        Some(mode),
        stream,
    )?;
    command.env("AIDA_HEADLESS_VENDOR", vendor.as_str());
    if wrapper {
        // Host-only routing metadata, excluded from the wrapper's child env.
        // Never downgrade to native if trusted policy changes after composition.
        command.env(CLAUDE_COORDINATOR_WRAPPER_ENV, "1");
    }
    Ok(command)
}

// Routing intent grants no containment credit; prepare revalidates trusted policy.
// trace:TASK-3219 | ai:codex
const CLAUDE_COORDINATOR_WRAPPER_ENV: &str = "AIDA_CLAUDE_COORDINATOR_WRAPPER";

/// The program and argv [`build_coordinator_headless_command`] would launch,
/// for a dry-run preview. Building the preview reserves no child handle,
/// probes no harness version and prepares no mailbox.
// trace:BUG-2389 | ai:claude
pub(crate) fn coordinator_headless_preview(
    host_main: Option<&Path>,
    vendor: HeadlessVendor,
    prompt: &str,
    mode: &str,
    seat: aida_core::agents_config::AgentSeat,
    contained: bool,
    stream: bool,
) -> Result<(String, Vec<String>)> {
    let root = headless_worktree_root();
    let tuning = resolve_agent_tuning(&root, vendor, seat);
    let prompt =
        crate::intake::materialize_skill_invocation(prompt, vendor == HeadlessVendor::Claude);
    let contained = aida_contained_env()? || contained;
    // Preview the same coordinator argv without preparing a boundary.
    // trace:TASK-3219 | ai:codex
    let _composition = (vendor == HeadlessVendor::Claude
        && contained_wrapper_launch(vendor, contained)?)
    .then(ClaudeWrapperComposition::enter);
    let (program, mut args) = compose_headless_command_for(
        host_main,
        vendor,
        &prompt,
        "preview",
        contained,
        tuning.model.as_deref(),
        tuning.effort.as_deref(),
        false,
    )?;
    // trace:TASK-2712 | ai:claude
    rescope_native_codex_for_seat(host_main, vendor, &mut args, &root, seat.as_str())?;
    adapt_coordinator_args(vendor, &mut args, Some(mode), contained, stream);
    Ok((program, args))
}

/// TASK-2712 (advisor ruling F2): a composed native Codex argv's write set
/// re-derived for the lane's own `tree` and `seat` role, through the one
/// role-aware helper, so a reviewer seat gets the gate ledger and every other
/// seat does not. Other vendors and non-native argv are left alone.
// trace:TASK-2712 | ai:claude
// trace:TASK-2032 | ai:claude
pub(crate) fn rescope_native_codex_for_seat(
    host_main: Option<&Path>,
    vendor: HeadlessVendor,
    args: &mut Vec<String>,
    tree: &Path,
    seat: &str,
) -> Result<()> {
    if vendor != HeadlessVendor::Codex {
        return Ok(());
    }
    crate::codex_native_sandbox::rescope_codex_native_flags(
        host_main,
        args,
        tree,
        Some(&crate::canonical_role_name(seat)),
    )
}

/// The permission mode a headless coordinator launches with. Claude resolves
/// the operator's `--permission-mode` (or the default) against containment.
/// Other vendors have no such flag, so passing one is refused rather than
/// silently ignored.
// trace:BUG-2389 | ai:claude
pub(crate) fn coordinator_permission_mode(
    vendor: HeadlessVendor,
    permission_mode: Option<&str>,
    contained: bool,
) -> Result<String> {
    if vendor == HeadlessVendor::Claude {
        return resolve_headless_claude_posture(permission_mode, contained);
    }
    if permission_mode.is_some() {
        anyhow::bail!(
            "--permission-mode is only supported when the configured vendor is claude (it is {})",
            vendor.as_str()
        );
    }
    Ok(String::new())
}

/// Turn the shared queue-phase argv into a coordinator's: Claude takes the
/// caller's permission mode (`dontAsk` when contained) and, for a quiet
/// launch, plain text output; a streaming launch with a mode adds partial
/// message events for burndown's live progress. A quiet Codex launch drops
/// `--json`.
// trace:BUG-2389 | ai:codex
// trace:BUG-2389 | ai:claude
fn adapt_coordinator_args(
    vendor: HeadlessVendor,
    args: &mut Vec<String>,
    mode: Option<&str>,
    contained: bool,
    stream: bool,
) {
    if vendor != HeadlessVendor::Claude {
        if !stream {
            args.retain(|arg| arg != "--json");
        }
        return;
    }
    if let Some(mode) = mode {
        let resolved = if contained { "dontAsk" } else { mode }.to_string();
        if let Some(i) = args.iter().position(|arg| arg == "--permission-mode") {
            if let Some(slot) = args.get_mut(i + 1) {
                *slot = resolved;
            }
        }
    }
    if !stream {
        if let Some(i) = args.iter().position(|arg| arg == "--output-format") {
            args.drain(i..(i + 2).min(args.len()));
        }
        args.retain(|arg| arg != "--verbose");
    } else if mode.is_some() && !args.iter().any(|a| a == "--include-partial-messages") {
        // Insert before `--verbose` so the prompt stays the trailing positional;
        // without `--verbose`, insert before the prompt.
        let i = args
            .iter()
            .position(|a| a == "--verbose")
            .unwrap_or(args.len().saturating_sub(1));
        args.insert(i, "--include-partial-messages".to_string());
    }
}

/// Codex finalization for a coordinator child (burndown, autopilot intake,
/// questions clarify): stamp the child's own user and project root, then
/// carry its delegated seat into Codex tool shells. A refusal is returned
/// before anything spawns; the caller revokes the child grant it issued.
/// Other vendors pass through untouched.
// trace:BUG-2398 | ai:claude
pub(crate) fn finalize_coordinator_launch(
    vendor: HeadlessVendor,
    cmd: &mut std::process::Command,
    project_root: &Path,
) -> Result<()> {
    if vendor != HeadlessVendor::Codex {
        return Ok(());
    }
    cmd.env("AIDA_USER", crate::current_user_id(None))
        .env("AIDA_PROJECT_ROOT", project_root);
    crate::vendors::profile::finalize_codex(cmd)
}

/// Finalize, then wait on a coordinator `cmd`, releasing the child handle
/// `guard` protects unless the child actually spawns. Disarm at spawn: a
/// running child owns the handle it was given. `stdout` is applied only after
/// finalization, which rebuilds the Command and would drop earlier stdio.
// trace:BUG-2389 | ai:claude
// trace:BUG-2398 | ai:claude
pub(crate) fn coordinator_status_with_handle_guard(
    vendor: HeadlessVendor,
    cmd: &mut std::process::Command,
    project_root: &Path,
    stdout: Option<std::fs::File>,
    mut guard: Option<crate::agent_handle::LaunchReservationGuard>,
) -> Result<std::process::ExitStatus> {
    use crate::process_retry::RetryEtxtbsy;
    use std::process::Stdio;
    finalize_coordinator_launch(vendor, cmd, project_root)?;
    // A coordinator is a foreground headless child. Retain the shared owner
    // until exit, and bind its explicit child grant before releasing the worker.
    // Resolve from the launch cwd, rather than the coordination-state root.
    // trace:TASK-3219 | ai:codex
    let root = cmd
        .get_current_dir()
        .map(Path::to_path_buf)
        .unwrap_or_else(headless_worktree_root);
    let explicit = |key: &str| {
        cmd.get_envs()
            .find(|(k, _)| *k == key)
            .and_then(|(_, v)| v)
            .and_then(|v| v.to_str())
            .map(str::to_owned)
    };
    if vendor == HeadlessVendor::Claude
        && (explicit(CLAUDE_COORDINATOR_WRAPPER_ENV).as_deref() == Some("1")
            || (explicit("AIDA_CONTAINED").as_deref() == Some("1")
                && claude_uses_contained_wrapper(&root)?))
    {
        let grant = explicit(crate::gate::seat_authority::GRANT_ENV)
            .context("contained coordinator requires an explicit child grant decision")?;
        cmd.env("AIDA_USER", crate::current_user_id(None))
            .env("AIDA_PROJECT_ROOT", &root);
        let host_main = crate::seat_clone::host_main_from(&root);
        let mut run =
            crate::containment::launch::PreparedLaunch::prepare(cmd, &root, host_main.as_deref())?
                .spawn(stdout.map(Stdio::from).unwrap_or_else(Stdio::inherit))?;
        anyhow::ensure!(
            containment_applied(vendor, true, Some(run.receipt())),
            "missing applied receipt"
        );
        if let Some(guard) = guard.as_mut() {
            guard.disarm();
        }
        crate::gate::seat_authority::bind_contained_launch_worker(&grant, run.receipt())?;
        run.release()?;
        let status = run.child.wait()?;
        return run.vendor_exit(status);
    }
    // AGY needs structured output even for quiet coordinators. Forward JSON
    // to streaming consumers; quiet callers receive only the final response.
    // Interactive AGY launches without stream-json retain their ordinary IO.
    // trace:TASK-2838 | ai:codex
    let agy_stream =
        vendor == HeadlessVendor::Agy && cmd.get_args().any(|arg| arg == "stream-json");
    let mut forward = stdout;
    if agy_stream {
        cmd.stdout(std::process::Stdio::piped());
    } else if let Some(stdout) = forward.take() {
        cmd.stdout(stdout);
    }
    let mut child = cmd.spawn_retrying_etxtbsy()?;
    if let Some(guard) = guard.as_mut() {
        guard.disarm();
    }
    if agy_stream {
        return Ok(crate::agy_outcome::coordinator_status(&mut child, forward)?);
    }
    Ok(child.wait()?)
}

// trace:BUG-2389 | ai:codex
#[allow(clippy::too_many_arguments)]
fn build_coordinator_headless_command_inner(
    host_main: Option<&Path>,
    vendor: HeadlessVendor,
    prompt: &str,
    session_id: &str,
    contained: bool,
    model: Option<&str>,
    effort: Option<&str>,
    seat: &str,
    grant_env: &str,
    resume: bool,
    mode: Option<&str>,
    stream: bool,
) -> Result<std::process::Command> {
    // trace:BUG-2172 | ai:codex
    let contained = aida_contained_env()? || contained;
    let (program, mut args) = compose_headless_command_for(
        host_main, vendor, prompt, session_id, contained, model, effort, resume,
    )?;
    // trace:TASK-2712 TASK-2032 | ai:claude
    rescope_native_codex_for_seat(
        host_main,
        vendor,
        &mut args,
        &headless_worktree_root(),
        seat,
    )?;
    adapt_coordinator_args(vendor, &mut args, mode, contained, stream);
    // TASK-1169 / ADR-22: same bounded ceiling as the spawn path — the exec
    // path is the `--no-human` phase-1 implementer, which must not diverge.
    // trace:TASK-1169 | ai:claude
    let (ceiling_key, ceiling_value) = crate::bg_wait_ceiling_env(Some(&headless_worktree_root()));
    // trace:TASK-2631 | ai:codex
    let posture = containment_posture(vendor, contained)?;
    // trace:STORY-2326 TASK-2032 | ai:claude
    // The exec and coordinator owners both prepare this command through the
    // receipt-owning wrapper. Keep ambient values implicit for its selector.
    // trace:STORY-2326 | ai:codex
    let wrapper = contained_wrapper_launch(vendor, contained)?;
    let ctx = headless_launch_context(host_main, vendor, program, args, posture)?
        .with_wrapper_environment(wrapper);
    let mut child_cmd = crate::launch::spawn_agent(&ctx)?;
    child_cmd
        .env("AIDA_HEADLESS", "1")
        // BUG-802: same drive-root anchor as the spawn path — the two launch
        // paths must never diverge on env.
        .env("AIDA_DRIVE_ROOT", headless_worktree_root())
        // BUG-806: same PATH rule as the spawn path.
        .env("PATH", drive_path_env().unwrap_or_default())
        .envs(crate::headless_wait_envs(ceiling_key, &ceiling_value));
    // BUG-2139: the child's seat authority is ALWAYS set explicitly — the
    // grant minted for this launch, or `""` for a baseline child — never
    // inherited from the launcher, whose grant may carry delegable seats.
    // trace:STORY-2326 | ai:claude
    crate::launch::assign_seat(
        &mut child_cmd,
        ctx.kind(),
        crate::launch::SeatDecision::Child {
            role: seat,
            grant: grant_env,
        },
    )?;
    // trace:BUG-2187 | ai:codex
    apply_child_agent_identity(&mut child_cmd, vendor, seat);
    // trace:TASK-1955 | ai:claude (TASK-1804)
    apply_headless_harness_version(host_main, &mut child_cmd, vendor, &headless_worktree_root());
    Ok(child_cmd)
}

// trace:BUG-2139 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn exec_vendor_headless(
    host_main: Option<&Path>,
    vendor: HeadlessVendor,
    prompt: &str,
    session_id: &str,
    log_path: &Path,
    tee_opts: &crate::headless_tee::TeeOptions,
    contained: bool,
    lease_id: Option<&str>,
    seat: &str,
    child_grant: &crate::gate::seat_authority::HeadlessChildGrant,
    lane_tree: Option<(&Path, &str)>,
    record_no_change: bool,
) -> Result<()> {
    let status = run_vendor_headless_to_exit(
        host_main,
        vendor,
        prompt,
        session_id,
        log_path,
        tee_opts,
        contained,
        lease_id,
        seat,
        child_grant,
        lane_tree,
        record_no_change,
    )?;
    std::process::exit(status.code().unwrap_or(1));
}

/// Wait for the headless child and settle host publication before returning
/// its status. Kept separate from process exit for launch integration tests.
// trace:STORY-2475 | ai:codex
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_vendor_headless_to_exit(
    host_main: Option<&Path>,
    vendor: HeadlessVendor,
    prompt: &str,
    session_id: &str,
    log_path: &Path,
    tee_opts: &crate::headless_tee::TeeOptions,
    contained: bool,
    lease_id: Option<&str>,
    seat: &str,
    child_grant: &crate::gate::seat_authority::HeadlessChildGrant,
    lane_tree: Option<(&Path, &str)>,
    record_no_change: bool,
) -> Result<std::process::ExitStatus> {
    // Revoke even when publication preparation or gate reservation fails.
    // trace:STORY-2640 trace:BUG-2139 | ai:codex
    let child_grant_guard = crate::standalone_implementer::ChildGrantGuard(child_grant);
    let drain_lane = drain_resume_lane(vendor, seat, lease_id, lane_tree);
    let host_published = vendor == HeadlessVendor::Codex && contained;
    let publish_native = std::cell::Cell::new(host_published);
    // trace:TASK-2902 | ai:codex
    if let Some((_, (tree, _))) = implementer_job_lane(seat, lease_id, lane_tree) {
        let main = crate::main_worktree_root_from(&headless_worktree_root());
        if vendor == HeadlessVendor::Codex {
            crate::native_publication::prepare(&main, tree)?;
        }
        // Prior-attempt signals remain until the final spawn boundary below.
    }

    // BUG-2383: the drain's phase-1 implementer gets the standalone launch's
    // lane job runner too, so it can run its targeted tests outside its
    // 60-second tool-call cap, and a lane that ran none is reported untested.
    // This process is the seat's host side (it launches the sandboxed
    // vendor), so the runner runs here. trace:BUG-2383 | ai:claude
    let main_root = crate::main_worktree_root_from(&headless_worktree_root());
    // trace:STORY-2640 | ai:codex
    let mut _fast_gate = implementer_job_lane(seat, lease_id, lane_tree)
        .map(|(_, (_, branch))| {
            crate::lane_fast_gate::reserve(&main_root, branch)
                .map(crate::lane_fast_gate::Reservation::with_result_line)
        })
        .transpose()?;
    let result = with_implementer_jobs(
        &main_root,
        seat,
        lease_id,
        lane_tree,
        prompt,
        crate::lane_job::offer,
        |prompt, apply_lane_env| {
            let run_once =
                |prompt: &str, log: &Path, resume: bool, hold: Option<&dyn Fn() -> bool>| {
                    run_vendor_headless_child(
                        None,
                        host_main,
                        vendor,
                        prompt,
                        session_id,
                        log,
                        tee_opts,
                        contained,
                        lease_id,
                        seat,
                        child_grant.env_value(),
                        &HeadlessChildHooks {
                            customize: Some(apply_lane_env),
                            before_spawn: Some(&|| {
                                if !resume {
                                    if let Some((lease, (tree, _))) =
                                        implementer_job_lane(seat, lease_id, lane_tree)
                                    {
                                        let scope = crate::list_leases(&main_root)
                                            .into_iter()
                                            .find(|l| l.id == lease)
                                            .context("publication lease disappeared")?
                                            .scope;
                                        crate::standalone_implementer::provision_attempt_signals(
                                            &main_root, tree, &scope,
                                        )?;
                                    }
                                }
                                Ok(())
                            }),
                            resume,
                            hold_reap: hold,
                            ..HeadlessChildHooks::default()
                        },
                    )
                };
            // BUG-050: the `--auto-complete` drain's phase-1 implementer gets the
            // standalone launch's resume safety net, through the same loop: a lane
            // that parks on its own build is waited for and resumed, bounded.
            // trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
            let result = match drain_lane {
                None => run_once(prompt, log_path, false, None),
                Some((lease, (worktree, branch))) => drain_lane_with_resume(
                    lease,
                    worktree,
                    branch,
                    prompt,
                    log_path,
                    |p, l, r, h| run_once(p, l, r, Some(h)),
                ),
            };
            // trace:STORY-2640 | ai:codex — gate before revoking the repair grant.
            let mut status = result?;
            if let Some((lease, (tree, branch))) = implementer_job_lane(seat, lease_id, lane_tree) {
                let Some(active_lease) = crate::list_leases(&main_root)
                    .into_iter()
                    .find(|l| l.id == lease)
                else {
                    crate::lane_fast_gate::missing_lease(&main_root, Some(branch));
                    publish_native.set(false);
                    anyhow::ensure!(!host_published, "publication lease {lease} is missing");
                    return Ok(status);
                };
                let scope = active_lease.scope;
                if host_published {
                    publish_native.set(crate::lane_fast_gate::native(
                        &main_root,
                        tree,
                        &scope,
                        lease,
                        branch,
                        status.success(),
                        |p| {
                            status = run_once(
                                p,
                                &log_path.with_extension("fast-fix.jsonl"),
                                true,
                                None,
                            )?;
                            Ok(status.success())
                        },
                    )?);
                } else if !crate::lane_fast_gate::veto(&main_root, &scope) {
                    let actual_branch = _fast_gate
                        .as_mut()
                        .context("fast gate reservation missing")?
                        .exit_branch(tree);
                    let branch = actual_branch.as_str();
                    if let Some((pr, _)) =
                        crate::standalone_implementer::lookup_pr(&main_root, branch)
                    {
                        let queued = crate::lane_fast_gate::pushed(
                            &main_root,
                            tree,
                            &scope,
                            lease,
                            branch,
                            pr,
                            vendor.as_str(),
                        )?;
                        eprintln!(
                            "fast_gate={}",
                            crate::lane_fast_gate::marker(&main_root, branch)
                        );
                        anyhow::ensure!(
                            !queued,
                            "fast gate failed; one rework round queued before review"
                        );
                    }
                }
            }
            Ok::<_, anyhow::Error>(status)
        },
    );
    drop(child_grant_guard);
    let status = result?;
    // trace:STORY-2475 | ai:codex
    if publish_native.get() {
        if let Some((lease, (tree, branch))) = implementer_job_lane(seat, lease_id, lane_tree) {
            let scope = crate::list_leases(&main_root)
                .into_iter()
                .find(|l| l.id == lease)
                .context("publication lease disappeared")?
                .scope;
            crate::lane_fast_gate::publication_result(
                &main_root,
                branch,
                crate::ship_from_seat::publish_queue_exit_recorded(
                    &main_root,
                    tree,
                    &scope,
                    lease,
                    status.success(),
                    record_no_change,
                ),
            )?;
        }
    }
    Ok(status)
}

/// Job eligibility is vendor-neutral; resume eligibility below remains Claude-only.
// trace:BUG-2383 | ai:codex
pub(crate) fn implementer_job_lane<'a>(
    seat: &str,
    lease_id: Option<&'a str>,
    lane_tree: Option<(&'a Path, &'a str)>,
) -> Option<(&'a str, (&'a Path, &'a str))> {
    lease_id
        .zip(lane_tree)
        .filter(|_| crate::canonical_role_name(seat) == "implementer")
}

/// The actual drain launch boundary: prompt/env, runner lifetime and result
/// evidence are shared by every implementer vendor, independently of resume.
// trace:BUG-2383 | ai:codex
pub(crate) fn with_implementer_jobs<T>(
    main_root: &Path,
    seat: &str,
    lease_id: Option<&str>,
    lane_tree: Option<(&Path, &str)>,
    prompt: &str,
    offer: impl FnOnce(&Path, &Path, &str) -> Option<crate::lane_job::LaneJobs>,
    run: impl FnOnce(&str, &dyn Fn(&mut std::process::Command)) -> T,
) -> T {
    let lane = implementer_job_lane(seat, lease_id, lane_tree);
    let jobs = lane.and_then(|(lease, (tree, _))| offer(main_root, tree, lease));
    let lease = lane.map(|(lease, _)| lease);
    let prompt = if jobs.is_some() {
        format!(
            "{prompt}{}",
            crate::standalone_implementer::LANE_JOB_CONTRACT
        )
    } else if lease.is_some() {
        format!(
            "{prompt}\n\nLocal test runner unavailable. Report untested=true in your final result."
        )
    } else {
        prompt.to_string()
    };
    let apply = |cmd: &mut std::process::Command| {
        if let Some(jobs) = &jobs {
            jobs.apply_selector(cmd);
        }
    };
    // Invalidate previous evidence before launching, including a failed spawn.
    if let Some(lease) = lease {
        crate::lane_job::record_untested(main_root, lease, true);
    }
    // Published on the lease, so every vendor's tool shell finds it through
    // its public selector. trace:BUG-2383 | ai:claude
    let runner = jobs
        .clone()
        .map(|lane| crate::lane_job::Runner::start_published(lane, main_root));
    let result = run(&prompt, &apply);
    let untested = runner.map(|r| r.stop().untested()).unwrap_or(true);
    if let Some(lease) = lease {
        crate::lane_job::record_untested(main_root, lease, untested);
        if let Some(line) = crate::lane_job::phase_test_warning(main_root, Some(lease)) {
            eprintln!("{line}");
        }
    }
    result
}

/// BUG-050: which [`exec_vendor_headless`] launches get the drain resume
/// loop: a resumable-vendor implementer lane that has both a lease and a seat
/// tree. Everything else (a reviewer or advisor seat, an Antigravity lane, a
/// launch with no lease or no lane tree) runs once.
// trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
// trace:BUG-2386 | ai:claude
pub(crate) fn drain_resume_lane<'a>(
    vendor: HeadlessVendor,
    seat: &str,
    lease_id: Option<&'a str>,
    lane_tree: Option<(&'a Path, &'a str)>,
) -> Option<(&'a str, (&'a Path, &'a str))> {
    lease_id
        .zip(lane_tree)
        .filter(|_| vendor_can_resume(vendor) && crate::canonical_role_name(seat) == "implementer")
}

/// BUG-2386: whether `vendor`'s sessions can be resumed (Claude by its
/// `--session-id`, Codex by its bound thread id). Agy cannot.
// trace:BUG-2386 | ai:claude
// trace:STORY-2539 | ai:codex
pub(crate) fn vendor_can_resume(vendor: HeadlessVendor) -> bool {
    vendor.profile().resume_args("", false).is_some()
}

/// BUG-050: run the `--auto-complete` drain's phase-1 implementer on `lease`
/// in the shared [`crate::headless_background::ResumeLoop`]. The lane is done
/// once it punted or held (its signal file exists) or its seat pushed past
/// the head it launched on; otherwise it is resumed only if it parked.
// trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
pub(crate) fn drain_lane_with_resume(
    lease: &str,
    worktree: &Path,
    branch: &str,
    prompt: &str,
    log_path: &Path,
    run_once: impl FnMut(&str, &Path, bool, &dyn Fn() -> bool) -> Result<std::process::ExitStatus>,
) -> Result<std::process::ExitStatus> {
    use crate::headless_background as bg;
    let ceiling = std::time::Duration::from_millis(
        crate::burndown_run::resolved_bg_wait_ceiling_ms(Some(&headless_worktree_root())),
    );
    let launch_head = bg::seat_head(worktree);
    let signalled = || {
        // trace:BUG-2488 | ai:codex
        let main = crate::main_worktree_root_from(worktree);
        let seat_signal = crate::list_leases(&main)
            .into_iter()
            .find(|l| l.id == lease)
            .is_some_and(|l| {
                crate::punt::signal_path(worktree, &l.scope).exists()
                    || crate::punt::hold_signal_path(worktree, &l.scope).exists()
            });
        seat_signal
            || [
                crate::punt::SIGNAL_FILE_ENV,
                crate::punt::HOLD_SIGNAL_FILE_ENV,
            ]
            .iter()
            .filter_map(std::env::var_os)
            .any(|p| !p.is_empty() && Path::new(&p).exists())
    };
    let lane = bg::ResumeLoop {
        can_resume: true,
        ceiling,
        lease_id: lease,
        log_path,
        on_waiting: &|| {},
        on_log: &|_| {},
    };
    let end = lane.run(prompt.to_string(), run_once, || {
        let pushed = bg::SeatState::read(worktree, branch).pushed_since(launch_head.as_deref());
        ((), signalled() || pushed == Some(true))
    })?;
    Ok(end.status)
}

/// BUG-2139: revoke a headless child's grant, warning (never failing) on error.
// trace:BUG-2139 | ai:claude
pub(crate) fn revoke_headless_child_grant(id: &str) {
    if let Err(e) = crate::gate::seat_authority::revoke_by_id(id) {
        eprintln!(
            "{} could not revoke headless child grant {}: {e:#} (it still expires with its parent)",
            "Warning:".yellow().bold(),
            &id[..id.len().min(8)]
        );
    }
}

/// BUG-2139: the seat env every headless child gets. `AIDA_SESSION_ROLE` and
/// [`crate::seat_authority::GRANT_ENV`] are set explicitly (never inherited), and the
/// launcher's `AIDA_SESSION_PURPOSE` is scrubbed (BUG-2153 class): it describes
/// the launcher's session, not the child's.
// trace:BUG-2139 | ai:claude
pub(crate) fn apply_headless_child_seat_env(
    cmd: &mut std::process::Command,
    seat: &str,
    grant_env: &str,
) {
    cmd.env("AIDA_SESSION_ROLE", seat)
        .env(crate::gate::seat_authority::GRANT_ENV, grant_env);
    scrub_launcher_session_env(cmd);
}

/// Attribution comes from this launch's vendor and seat, never the launcher
/// (headless and interactive child launches alike).
/// These labels confer no authority: recording still requires its existing
/// reviewer grant and independent-session attestation.
///
/// Every seat gets it, not only the reviewer, deliberately: an implementer
/// child launched from an `aida agent new` lane is labelled
/// `<vendor>-implementer` (its own handle rides in `AIDA_HANDLE`) rather
/// than inheriting the lane's name. Inheriting would attribute the child's writes to whatever seat launched it (an
/// advisor's `queue work`, say). Lane attribution in `aida ps` does not
/// depend on this label: BUG-2010 binds a lane by the lease the child
/// claims itself, and skips children that claim none.
// trace:BUG-2187 | ai:codex
// trace:TASK-1955 | ai:claude (TASK-2066, TASK-1672)
pub(crate) fn apply_child_agent_identity(
    cmd: &mut std::process::Command,
    vendor: HeadlessVendor,
    seat: &str,
) {
    apply_child_identity(cmd, vendor, seat, None);
}

/// [`apply_child_agent_identity`] for a resume of native session
/// `session_id`: the session keeps the handle it already holds, and no new
/// one is reserved.
// trace:STORY-1843 | ai:claude
pub(crate) fn apply_resumed_child_agent_identity(
    cmd: &mut std::process::Command,
    vendor: HeadlessVendor,
    seat: &str,
    session_id: &str,
) {
    apply_child_identity(cmd, vendor, seat, Some(session_id));
}

fn apply_child_identity(
    cmd: &mut std::process::Command,
    vendor: HeadlessVendor,
    seat: &str,
    resumed_session: Option<&str>,
) {
    let agent_type = match vendor {
        HeadlessVendor::Claude => "claude",
        HeadlessVendor::Codex => "codex",
        HeadlessVendor::Agy => "antigravity",
    };
    // `AIDA_AGENT_NAME` is the seat identity review attestation records.
    // The child's own address is a separate handle in `AIDA_HANDLE`:
    // reserved here and bound by the child with the launch token, so
    // concurrent children of one seat never share a From address; a resume
    // keeps the handle its session holds. Without a project to reserve in,
    // no handle is exported and session start allocates one.
    // trace:STORY-1843 | ai:claude
    // trace:STORY-1843 | ai:codex — a child never borrows its parent session.
    for key in crate::agent_handle::INHERITED_SESSION_ENV {
        cmd.env_remove(key);
    }
    let handle = match resumed_session {
        Some(sid) => {
            child_handle_root().and_then(|root| crate::agent_handle::handle_of_session(&root, sid))
        }
        None => reserve_child_handle(cmd, agent_type, seat),
    };
    if let Some(handle) = handle {
        cmd.env(crate::agent_handle::HANDLE_ENV, handle);
    }
    cmd.env("AIDA_AGENT_TYPE", agent_type)
        .env("AIDA_AGENT_NAME", format!("{agent_type}-{seat}"));
}

/// Reserve a generated `<vendor>-<seat>-NN` handle for a child about to be
/// launched and hand it the launch token. Fail-open: `None` when no project
/// namespace is reachable.
// trace:STORY-1843 | ai:claude
fn reserve_child_handle(
    cmd: &mut std::process::Command,
    agent_type: &str,
    seat: &str,
) -> Option<String> {
    let root = child_handle_root()?;
    let reservation = crate::agent_handle::reserve_handle(
        &root,
        &crate::agent_handle::HandleRequest {
            agent_type,
            role: Some(seat),
            launcher: true,
            ..Default::default()
        },
    )
    .ok()?;
    let token = crate::agent_handle::launch_token_for(&root, &reservation.handle)?;
    cmd.env(crate::agent_handle::LAUNCH_TOKEN_ENV, token);
    Some(reservation.handle)
}

// trace:STORY-1843 | ai:codex
pub(crate) fn child_reservation_guard() -> Option<crate::agent_handle::LaunchReservationGuard> {
    child_handle_root().map(|root| crate::agent_handle::LaunchReservationGuard::new(&root))
}

// Disarm at spawn, not at wait completion: a wait/receipt error must not
// release a reservation already handed to a running child.
// trace:STORY-1843 | ai:codex
fn reviewer_status_with_handle_guard(
    cmd: &mut std::process::Command,
    guard: &mut Option<crate::agent_handle::LaunchReservationGuard>,
) -> std::io::Result<std::process::ExitStatus> {
    crate::review_watch::reviewer_status_on_spawn(cmd, || {
        if let Some(guard) = guard {
            guard.disarm();
        }
    })
}

/// The handle namespace a headless child is reserved in: the main checkout
/// of the launch's project. Tests reserve only in a pinned project.
fn child_handle_root() -> Option<PathBuf> {
    #[cfg(test)]
    {
        contained_cwd_seam::cwd()
    }
    #[cfg(not(test))]
    {
        let root = crate::find_aida_project_root_from(&headless_worktree_root()).ok()?;
        Some(crate::main_worktree_root_from(&root))
    }
}

/// Stamp `AIDA_HARNESS_VERSION` on a headless child from its own vendor
/// binary's `--version`, probed under the same os_wrap boundary as the child
/// (the #2710 probe: grant-scrubbed and bounded). A mocked vendor
/// (`AIDA_AGENT_CMD`) or an unwrappable probe clears the inherited value
/// rather than leaving the launcher's version on the child.
// trace:TASK-1955 | ai:claude (TASK-1804)
fn apply_headless_harness_version(
    host_main: Option<&Path>,
    cmd: &mut std::process::Command,
    vendor: HeadlessVendor,
    worktree_root: &Path,
) {
    let mocked = std::env::var("AIDA_AGENT_CMD").is_ok_and(|c| !c.trim().is_empty());
    let probe = (!mocked)
        .then(|| {
            os_wrapped_program_and_args(
                host_main,
                worktree_root,
                vendor.program(),
                vec!["--version".to_string()],
            )
            .ok()
        })
        .flatten();
    match probe {
        Some((program, args)) => {
            crate::agent_launch::apply_harness_version(cmd, (program.into(), args))
        }
        None => {
            cmd.env_remove("AIDA_HARNESS_VERSION");
        }
    }
}

/// BUG-2139: strip the launcher-session env a child must not inherit.
///
/// STORY-1662: the standalone-implementer marker is stripped too, so it
/// binds only the child its launcher set it on, never a grandchild that
/// child launches through AIDA.
// trace:BUG-2139 | ai:claude
pub(crate) fn scrub_launcher_session_env(cmd: &mut std::process::Command) {
    cmd.env_remove("AIDA_SESSION_PURPOSE");
    // trace:STORY-1662 | ai:claude
    cmd.env_remove(crate::standalone_implementer::MARKER_ENV);
    // A child lane gets its own lane marker. trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
    cmd.env_remove(crate::headless_background::LANE_MARKER_ENV);
    // Nor the launcher's named seat: the child's seat is its own registry
    // row's. trace:STORY-2745 | ai:claude
    cmd.env_remove(crate::agent_rotation::SEAT_ENV);
}

/// STORY-1662: per-launch additions to a headless child. `customize` runs on
/// the fully built command after the seat env (so it can add to or override
/// it); `on_spawn` receives the spawned child's pid.
// trace:STORY-1662 | ai:claude
#[derive(Default)]
pub(crate) struct HeadlessChildHooks<'a> {
    pub(crate) customize: Option<&'a dyn Fn(&mut std::process::Command)>,
    pub(crate) on_spawn: Option<&'a dyn Fn(u32)>,
    /// Consume prior-attempt markers only after command policy/prepare succeeds.
    pub(crate) before_spawn: Option<&'a dyn Fn() -> Result<()>>,
    /// BUG-044: `--resume` the existing `session_id` instead of starting it
    /// (Claude only; the launcher never asks another vendor to resume).
    pub(crate) resume: bool,
    /// BUG-044: while true, a vendor still alive after its final result is
    /// left running rather than reaped (its own background build is going).
    pub(crate) hold_reap: Option<&'a dyn Fn() -> bool>,
}

/// BUG-044: `args` (a composed `--session-id <id>` launch) rewritten to
/// `--resume <id>`, every other flag kept, so a resume runs with the same
/// posture, model and wrapper as the launch it continues.
// trace:BUG-B6B1278890774EF993D2EBC327862470-044 | ai:claude
pub(crate) fn resume_session_args(mut args: Vec<String>, session_id: &str) -> Result<Vec<String>> {
    let at = args
        .windows(2)
        .position(|w| w[0] == "--session-id" && w[1] == session_id)
        .ok_or_else(|| anyhow::anyhow!("no `--session-id {session_id}` to resume"))?;
    args[at] = "--resume".to_string();
    Ok(args)
}

/// STORY-1662: run a standalone implementer's headless child and wait for
/// it. Unlike [`exec_vendor_headless`] this returns the exit status, so the
/// launcher can classify the outcome. The launcher revokes the child's grant
/// with [`revoke_standalone_implementer_grant`] once it stops resuming it.
// trace:STORY-1662 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_standalone_implementer_child(
    project_root: &Path,
    host_main: Option<&Path>,
    vendor: HeadlessVendor,
    prompt: &str,
    session_id: &str,
    log_path: &Path,
    tee_opts: &crate::headless_tee::TeeOptions,
    contained: bool,
    lease_id: &str,
    child_grant: &crate::gate::seat_authority::HeadlessChildGrant,
    hooks: &HeadlessChildHooks<'_>,
) -> Result<std::process::ExitStatus> {
    run_vendor_headless_child(
        Some(project_root),
        host_main,
        vendor,
        prompt,
        session_id,
        log_path,
        tee_opts,
        contained,
        Some(lease_id),
        "implementer",
        child_grant.env_value(),
        hooks,
    )
}

/// The standalone implementer's grant dies with its last run, as on the
/// exec path.
// trace:BUG-B6B1278890774EF993D2EBC327862470-044 | ai:claude
pub(crate) fn revoke_standalone_implementer_grant(
    child_grant: &crate::gate::seat_authority::HeadlessChildGrant,
) {
    if let Some(id) = child_grant.id() {
        revoke_headless_child_grant(id);
    }
}

// trace:BUG-2139 | ai:claude
#[allow(clippy::too_many_arguments)]
fn run_vendor_headless_child(
    lease_root: Option<&Path>,
    host_main: Option<&Path>,
    vendor: HeadlessVendor,
    prompt: &str,
    session_id: &str,
    log_path: &Path,
    tee_opts: &crate::headless_tee::TeeOptions,
    contained: bool,
    lease_id: Option<&str>,
    seat: &str,
    grant_env: &str,
    hooks: &HeadlessChildHooks<'_>,
) -> Result<std::process::ExitStatus> {
    use std::process::Stdio;
    // BUG-2144: refuse before the log file and tee exist. trace:BUG-2144 | ai:claude
    require_vendor_containment(vendor, contained, "implementer session")?;
    if let Some(dir) = log_path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
    }
    let log = std::fs::File::create(log_path)
        .with_context(|| format!("failed to create headless log {}", log_path.display()))?;
    let tee = crate::headless_tee::start_tee(log_path, tee_opts);
    // BUG-705: this exec path used to compose the claude argv directly and
    // never consulted the headless vendor, so a codex-routed drain's phase-1
    // implementer always tried claude (fatal on a claude-less machine). It now
    // shares the spawn path's per-vendor composition; the claude arm is
    // byte-identical to the spawn path's claude arm.
    if vendor != HeadlessVendor::Claude {
        eprintln!(
            "{} headless implementer phase on vendor `{}`",
            "Vendor:".cyan().bold(),
            vendor.as_str()
        );
    }
    let tuning = resolve_agent_tuning(
        &headless_worktree_root(),
        vendor,
        aida_core::agents_config::AgentSeat::Implementer,
    );
    // BUG-2386: a Codex resume continues the thread Codex minted, not the
    // AIDA id; resolve it (lease stamp, headless log, unique rollout) or
    // refuse by name. trace:BUG-2386 | ai:claude
    let resume_id = if hooks.resume && vendor == HeadlessVendor::Codex {
        let project_root = crate::find_project_root()?;
        let worktree = headless_worktree_root();
        crate::codex_resume::resolve_thread_id(&crate::codex_resume::ThreadLookup {
            project_root: &project_root,
            session_id,
            lease_id,
            worktree: &worktree,
            since: None,
        })?
    } else {
        session_id.to_string()
    };
    // Own reservations from command construction through the actual spawn.
    // trace:STORY-1843 | ai:codex
    let mut handle_guard = child_reservation_guard();
    // trace:TASK-2708 | ai:claude — a Claude resume continues a session the
    // lane's fresh private config dir does not hold: it stays outside the
    // wrapper scope, so under `claude = "aida"` it refuses by name.
    let wrapper = contained_wrapper_launch(vendor, contained)?;
    let composition = (wrapper && vendor == HeadlessVendor::Claude && !hooks.resume)
        .then(ClaudeWrapperComposition::enter);
    let mut child_cmd = build_exec_vendor_headless_command_for(
        host_main,
        vendor,
        prompt,
        &resume_id,
        contained,
        tuning.model.as_deref(),
        tuning.effort.as_deref(),
        seat,
        grant_env,
        hooks.resume,
    )?;
    drop(composition);
    // trace:STORY-1662 | ai:claude
    if let Some(customize) = hooks.customize {
        customize(&mut child_cmd);
    }
    // The drain parent carries host paths; the vendor can write only its seat.
    // trace:BUG-2488 | ai:codex
    if let Some(id) = lease_id
        .filter(|_| lease_root.is_none() && crate::canonical_role_name(seat) == "implementer")
    {
        let main = crate::main_worktree_root_from(&headless_worktree_root());
        let lease = crate::list_leases(&main)
            .into_iter()
            .find(|l| l.id == id)
            .context("implementer lease disappeared before signal transport")?;
        crate::standalone_implementer::apply_seat_signals(
            &mut child_cmd,
            &lease.worktree_path,
            &lease.scope,
        );
    }
    // trace:TASK-3225 | ai:codex — an explicit requirement cannot downgrade to native.
    crate::containment::launch::require_store_service_surface(&child_cmd, wrapper)?;
    // BUG-050: name the lane's processes, so its launcher finds the build it
    // left running (standalone and drain alike).
    // trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
    if let Some(id) = lease_id {
        child_cmd.env(crate::headless_background::LANE_MARKER_ENV, id);
    }
    if vendor == HeadlessVendor::Codex || wrapper {
        child_cmd
            .env("AIDA_USER", crate::current_user_id(None))
            .env("AIDA_PROJECT_ROOT", headless_worktree_root());
        if let Some(id) = lease_id {
            child_cmd.env(crate::gate::review_authority::WORKER_SESSION_ENV, id);
        }
    }
    if vendor == HeadlessVendor::Codex
        && containment_posture(vendor, contained)? != ContainmentPosture::Aida
    {
        crate::vendors::profile::finalize_codex(&mut child_cmd)?;
    }
    // trace:TASK-2707 | ai:codex — the foreground owner retains private state,
    // broker and proxy until the wrapper is reaped. No model before receipt.
    // trace:TASK-2708 | ai:claude — Claude under `claude = "aida"` too.
    crate::reviewer_completion::bind_grant(grant_env); // trace:BUG-2546 | ai:codex
    if wrapper {
        // trace:TASK-2708 | ai:claude — every exit past the tee start, success
        // or error, finishes the lane record it registered and stops the tee;
        // a dropped ContainedChild kills the wrapper.
        let mut registered = false;
        let result = (|| -> Result<std::process::ExitStatus> {
            let prepared = crate::containment::launch::PreparedLaunch::prepare(
                &mut child_cmd,
                &headless_worktree_root(),
                host_main,
            )?;
            if let Some(before_spawn) = hooks.before_spawn {
                before_spawn()?;
            }
            let mut run = prepared.spawn(Stdio::from(log))?;
            anyhow::ensure!(
                containment_applied(vendor, true, Some(run.receipt())),
                "missing applied receipt"
            );
            let worker = run.receipt().worker_pid();
            if let Some(guard) = &mut handle_guard {
                guard.disarm();
            }
            if let Some(on_spawn) = hooks.on_spawn {
                on_spawn(worker);
            }
            if let Some(id) = lease_id {
                stamp_lease_active_pid(id, worker)?;
            }
            crate::lane_liveness::register_headless_lane_for_child(
                log_path,
                lease_id,
                vendor.as_str(),
                worker,
            );
            registered = true;
            // trace:TASK-2708 | ai:claude
            crate::gate::seat_authority::bind_contained_launch_worker(grant_env, run.receipt())?;
            run.release()?;
            crate::headless_linger::wait_reaping_after_result_holding(
                &mut run.child,
                log_path,
                crate::headless_linger::result_grace(),
                hooks.hold_reap.unwrap_or(&|| false),
            )
            .map(|w| w.status)
            .with_context(|| format!("failed to wait for contained {}", vendor.program()))
            .and_then(|status| run.vendor_exit(status))
        })();
        if registered {
            crate::lane_liveness::finish_headless_lane_for_child(
                log_path,
                lease_id,
                result.as_ref().ok().copied(),
            );
        }
        tee.stop();
        return result;
    }
    // trace:STORY-2537 trace:BUG-2458 | ai:codex
    #[cfg(target_os = "linux")]
    let holder_capture = aida_core::proc_identity::prepare_command(&mut child_cmd)?;
    if let Some(before_spawn) = hooks.before_spawn {
        if let Err(error) = before_spawn() {
            tee.stop();
            return Err(error);
        }
    }
    let mut child = child_cmd
        .stdout(Stdio::from(log))
        .spawn_retrying_etxtbsy()
        .with_context(|| format!("failed to spawn {}", vendor.program()))?;
    if let Some(guard) = &mut handle_guard {
        guard.disarm();
    }
    if let Some(on_spawn) = hooks.on_spawn {
        on_spawn(child.id());
    }
    #[cfg(target_os = "linux")]
    if let Some(id) = lease_id {
        let holder = holder_capture.finish(&mut child);
        // Resolve from the explicit launch context, never the launcher's cwd:
        // standalone/rework launches can run for another linked checkout.
        // trace:STORY-2537 trace:BUG-2458 | ai:codex
        let worker_cwd = child_cmd
            .get_current_dir()
            .map(Path::to_path_buf)
            .unwrap_or_else(headless_worktree_root);
        let stamped = lease_root
            .or(host_main)
            .map(Path::to_path_buf)
            .map(Ok)
            .unwrap_or_else(|| crate::find_project_root_from(&worker_cwd))
            .map(|root| crate::main_worktree_root_from(&root))
            .and_then(|root| stamp_lease_holder_at(&root, id, child.id(), &holder));
        if let Err(error) = stamped {
            // The retained Child pins this PID. A failed handoff must not
            // leave the just-spawned worker running without its lease stamp.
            let _ = child.kill();
            let _ = child.wait();
            tee.stop();
            return Err(error.context("could not establish the spawned worker's holder generation"));
        }
    }
    // trace:STORY-1688 | ai:claude
    crate::lane_liveness::register_headless_lane_for_child(
        log_path,
        lease_id,
        vendor.as_str(),
        child.id(),
    );
    // A vendor still alive after its final result (held up by an MCP child)
    // is terminated past a bounded grace, so it frees its lane and cap slot.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-027 | ai:claude
    // trace:BUG-B6B1278890774EF993D2EBC327862470-044 | ai:claude
    let status = crate::headless_linger::wait_reaping_after_result_holding(
        &mut child,
        log_path,
        crate::headless_linger::result_grace(),
        hooks.hold_reap.unwrap_or(&|| false),
    )
    .map(|waited| {
        if waited.reaped {
            eprintln!(
                "{} {} was still running {}s after its final result; terminated it and its children",
                "Note:".cyan().bold(),
                vendor.program(),
                crate::headless_linger::result_grace().as_secs()
            );
        }
        waited.status
    })
    .with_context(|| format!("failed to wait for {}", vendor.program()));
    // trace:STORY-1688 | ai:claude
    crate::lane_liveness::finish_headless_lane_for_child(
        log_path,
        lease_id,
        status.as_ref().ok().copied(),
    );
    // BUG-2386: bind the thread a fresh Codex run started to its lease, so a
    // later resume (background wait, `queue work --resume`) continues it.
    // trace:BUG-2386 | ai:claude
    if vendor == HeadlessVendor::Codex && !hooks.resume {
        if let (Some(lease), Ok(root)) = (lease_id, crate::find_project_root()) {
            crate::codex_resume::bind_thread_from_log(&root, lease, log_path);
        }
    }
    let status = status?;
    tee.stop();
    // trace:TASK-2838 | ai:codex
    Ok(if vendor == HeadlessVendor::Agy {
        crate::agy_outcome::log_status(log_path, status)
    } else {
        status
    })
}

/// STORY-306: build the argv (after the `claude` program name) for a headless
/// `claude -p --resume <id>` launch — the advisor tier's implementer-resume
/// leg. Identical to [`claude_headless_args_with_posture`] (the SPIKE-7 mandatory flag
/// set) except it `--resume`s an existing session instead of minting a new
/// `--session-id`, so the resumed implementer re-enters its punted phase-1
/// conversation with the working model it had already built. Pure — the flag
/// set is unit-tested without spawning claude. trace:STORY-306 | ai:claude
pub fn claude_headless_resume_args(prompt: &str, session_id: &str) -> Vec<String> {
    // Uncontained: no contained settings are built, so nothing can refuse.
    claude_headless_resume_args_with_posture(None, prompt, session_id, None).unwrap_or_default()
}

/// `contained_in` is `Some(worktree)` for a contained resume: the write
/// boundary is scoped to that worktree (the resumed session's cwd), not to the
/// process cwd. `None` is the uncontained `bypassPermissions` posture.
// trace:BUG-1971 | ai:claude
pub fn claude_headless_resume_args_with_posture(
    host_main: Option<&Path>,
    prompt: &str,
    session_id: &str,
    contained_in: Option<&Path>,
) -> Result<Vec<String>> {
    let permission_mode = if contained_in.is_some() {
        "dontAsk"
    } else {
        "bypassPermissions"
    };
    let mut args = vec![
        "-p".to_string(),
        "--permission-mode".to_string(),
        permission_mode.to_string(),
    ];
    let contained_flags = match contained_in {
        // BUG-1971: only when `worktree` is itself a git root; otherwise no
        // write roots. trace:BUG-1971 | ai:claude
        Some(worktree) => {
            claude_contained_flags_for_roots(host_main, &contained_resume_write_roots(worktree))?
        }
        None => Vec::new(),
    };
    // BUG-050: a resumed lane is as headless as the launch it continues.
    // BUG-327: the posture's `--disallowed-tools` list is variadic; the
    // `--output-format` after it ends the list, so the trailing prompt
    // positional is unharmed. trace:BUG-327 | ai:claude
    // trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
    args.extend(headless_claude_posture(contained_flags)?);
    args.extend([
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--resume".to_string(),
        session_id.to_string(),
        prompt.to_string(),
    ]);
    Ok(args)
}

/// TASK-894: build the `(program, args)` for one launch of the headless
/// **advisor tier** (`/aida-advise`) the orchestrator spawns to resolve an
/// implementer's punt under `--auto-complete --no-human=both`. Vendor-neutral
/// generalization of the STORY-306 spawn, mirroring STORY-683's drain-phase
/// generalization.
///
/// - `is_fork` is the orchestrator's fork-from-live decision for the Claude
///   path: when `true` the Claude arm resumes the forked live-advisor session
///   (`claude … --resume <advisor_uuid> /aida-advise`, the context-rich path);
///   when `false` it cold-boots with the substrate-seeded prompt.
/// - `seeded_prompt` is the cold-boot prompt (the live-advisor-context prepend
///   the assess cold-boot uses); `advisor_uuid` is the resume/session id.
///
/// **Codex has no `--resume` / session model** (see
/// [`codex_headless_args_with_model_and_effort`]), so
/// a Codex advisor tier ignores `is_fork` and always hosts a *fresh* `codex exec`
/// per punt against the seeded prompt — no resume, the per-punt-fresh-spawn
/// trade-off noted in STORY-683's follow-ups. The caller is responsible for
/// forcing the pass to cold-boot for a non-Claude vendor so the fork-from-live
/// JSONL machinery (Claude-specific) is never exercised.
///
/// Claude's `is_fork`/cold-boot arms are byte-identical to the pre-existing
/// inline construction, so an un-configured drain (vendor = Claude) is unchanged.
/// Pure — both arms are unit-tested without spawning.
// trace:TASK-894 | ai:claude
// trace:TASK-1581 | ai:antigravity — test-only default-tuning wrapper; the
// orchestrator calls `advisor_tier_program_and_args_with_tuning`.
#[cfg(test)]
pub fn advisor_tier_program_and_args(
    vendor: HeadlessVendor,
    is_fork: bool,
    seeded_prompt: &str,
    advisor_uuid: &str,
) -> (String, Vec<String>) {
    advisor_tier_program_and_args_with_tuning(
        vendor,
        is_fork,
        seeded_prompt,
        advisor_uuid,
        None,
        None,
    )
}

// trace:STORY-1033 | ai:codex
pub fn advisor_tier_program_and_args_with_tuning(
    vendor: HeadlessVendor,
    is_fork: bool,
    seeded_prompt: &str,
    advisor_uuid: &str,
    model: Option<&str>,
    effort: Option<&str>,
) -> (String, Vec<String>) {
    // Uncontained: no contained settings are built (so no host main is
    // read), and nothing can refuse.
    advisor_tier_program_and_args_with_posture(
        None,
        vendor,
        is_fork,
        seeded_prompt,
        advisor_uuid,
        model,
        effort,
        None,
    )
    .unwrap_or_else(|_| (resolve_agent_program(vendor.program()), Vec::new()))
}

/// BUG-2133: the advisor tier's `(program, args)` with the drain's
/// containment threaded in. `contained_in = Some(dir)` (the advisor child's
/// cwd) gives the Claude arms, fork and cold-boot alike, the contained posture
/// scoped to `dir`; `None` is the uncontained posture. Non-Claude arms cannot
/// be contained — the caller refuses them first via
/// [`require_child_containment`].
// trace:BUG-2133 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn advisor_tier_program_and_args_with_posture(
    host_main: Option<&Path>,
    vendor: HeadlessVendor,
    is_fork: bool,
    seeded_prompt: &str,
    advisor_uuid: &str,
    model: Option<&str>,
    effort: Option<&str>,
    contained_in: Option<&Path>,
) -> Result<(String, Vec<String>)> {
    Ok(match vendor {
        HeadlessVendor::Claude => {
            let args = if is_fork {
                // Fork branch inherits the live advisor's context via --resume.
                claude_headless_resume_args_with_posture(
                    host_main,
                    "/aida-advise",
                    advisor_uuid,
                    contained_in,
                )?
            } else {
                let roots = contained_in.map(contained_resume_write_roots);
                claude_headless_args_for_roots(
                    host_main,
                    seeded_prompt,
                    advisor_uuid,
                    roots.as_deref(),
                    model,
                    effort,
                )?
            };
            // TASK-1081: route the vendor binary through the mock resolver — an
            // `AIDA_AGENT_CMD` override swaps the program, argv unchanged; unset
            // yields the native binary. trace:TASK-1081
            (
                resolve_agent_program(HeadlessVendor::Claude.program()),
                args,
            )
        }
        HeadlessVendor::Codex => {
            // A fresh `codex exec` per punt against the seeded prompt, or, when
            // `is_fork`, `codex exec fork <advisor_uuid> <prompt>`: the caller
            // passes the registered live Codex thread as `advisor_uuid`. The
            // fork keeps the fresh launch's posture flags (contained or not).
            // trace:TASK-894 | ai:claude
            // trace:STORY-2255 | ai:claude
            // TASK-1081: same mock resolver as the Claude arm. trace:TASK-1081
            let args = headless_vendor_args_contained_in(
                host_main,
                vendor,
                seeded_prompt,
                advisor_uuid,
                contained_in,
                model,
                effort,
            )?;
            let args = if is_fork {
                codex_exec_fork_args(args, advisor_uuid)?
            } else {
                args
            };
            (resolve_agent_program(HeadlessVendor::Codex.program()), args)
        }
        HeadlessVendor::Agy => {
            // AGY, like Codex, has no AIDA-addressable resume/session model, so
            // the advisor tier hosts a fresh `agy -p` per punt against the
            // seeded prompt; `is_fork` is intentionally ignored.
            // TASK-1081: same mock resolver as the other arms. trace:TASK-1048
            (
                resolve_agent_program(HeadlessVendor::Agy.program()),
                agy_headless_args_with_effort(seeded_prompt, effort),
            )
        }
    })
}

/// STORY-306: spawn a headless `claude -p --resume <id>` run and wait,
/// returning the exit status. The spawn-and-wait counterpart of
/// [`spawn_claude_headless`] for the orchestrator's advisor-resume leg — the
/// parent stays alive to classify the resumed implementer's outcome. Claude's
/// stream-json stdout is redirected to `log_path`; `AIDA_HEADLESS=1` is set so
/// the resumed skill knows it is unattended.
///
/// `cwd` must be the working directory the original session was created in
/// (typically the implementer's worktree). Claude Code persists each session
/// at `~/.claude/projects/<cwd-slug>/<session-id>.jsonl` — the slug is
/// derived from cwd, so a resume from a different directory looks in the
/// wrong slug folder and fails with "No conversation found." The original
/// SPIKE-7 verification of this code missed this because the unit test
/// exercises only the argv. trace:STORY-306 | ai:claude
#[allow(clippy::too_many_arguments)]
pub fn spawn_claude_headless_resume(
    host_main: Option<&Path>,
    prompt: &str,
    session_id: &str,
    log_path: &Path,
    cwd: &Path,
    tee_opts: &crate::headless_tee::TeeOptions,
    contained: bool,
    seat: &str,
) -> Result<std::process::ExitStatus> {
    spawn_claude_headless_resume_granted(
        host_main, prompt, session_id, log_path, cwd, tee_opts, contained, seat,
    )
    .map(|(status, _)| status)
}

/// BUG-2149: [`spawn_claude_headless_resume`], also returning the id of the
/// child seat grant it issued.
// trace:BUG-2149 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_claude_headless_resume_granted(
    host_main: Option<&Path>,
    prompt: &str,
    session_id: &str,
    log_path: &Path,
    cwd: &Path,
    tee_opts: &crate::headless_tee::TeeOptions,
    contained: bool,
    seat: &str,
) -> Result<(std::process::ExitStatus, String)> {
    use std::process::{Command, Stdio};
    // trace:BUG-2172 | ai:codex
    let contained = aida_contained_env()? || contained;
    if let Some(dir) = log_path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
    }
    let log = std::fs::File::create(log_path)
        .with_context(|| format!("failed to create headless log {}", log_path.display()))?;
    let tee = crate::headless_tee::start_tee(log_path, tee_opts);
    // TASK-1169 / ADR-22: the resume path is a headless child too — same
    // bounded ceiling, so a resumed drain can't diverge from a fresh one.
    // trace:TASK-1169 | ai:claude
    let (ceiling_key, ceiling_value) = crate::bg_wait_ceiling_env(Some(cwd));
    // STORY-612: OS-boundary wrapper. The resume path runs in `cwd` (the
    // original implementer's worktree), so that is the rw bind scope.
    // trace:STORY-612 | ai:claude
    let (program, args) = claude_program_and_args(
        host_main,
        cwd,
        // BUG-1971: the write boundary is the worktree being resumed in.
        // trace:BUG-1971 | ai:claude
        claude_headless_resume_args_with_posture(
            host_main,
            prompt,
            session_id,
            contained.then_some(cwd),
        )?,
    )?;
    // trace:BUG-2218 TASK-2328 | ai:codex
    if contained {
        prepare_contained_mailbox_for_launch(host_main, cwd)?;
    }
    // A contained resume is sandboxed like a fresh launch: refuse a grant
    // its tool calls could not present. trace:BUG-b6b1278890774ef993d2ebc327862470-045 | ai:claude
    let child_grant =
        crate::launch::issue_launch_child(cwd, seat, &crate::current_user_id(None), contained)?;
    let mut cmd = Command::new(program);
    cmd.current_dir(cwd)
        .args(args)
        .env("AIDA_HEADLESS", "1")
        .env("AIDA_SESSION_ROLE", seat)
        .env(crate::gate::seat_authority::GRANT_ENV, &child_grant.id)
        // BUG-802: same drive-root anchor as the spawn path — the two launch
        // paths must never diverge on env.
        .env("AIDA_DRIVE_ROOT", headless_worktree_root())
        .envs(crate::headless_wait_envs(ceiling_key, &ceiling_value));
    // A resume launches Claude even when the launcher is another harness.
    // trace:BUG-2187 | ai:codex
    // trace:STORY-1843 | ai:codex
    let mut handle_guard = child_reservation_guard();
    apply_resumed_child_agent_identity(&mut cmd, HeadlessVendor::Claude, seat, session_id);
    // trace:TASK-1955 | ai:claude (TASK-1804)
    apply_headless_harness_version(host_main, &mut cmd, HeadlessVendor::Claude, cwd);
    // trace:BUG-2139 | ai:claude
    scrub_launcher_session_env(&mut cmd);
    // trace:BUG-2006 | ai:antigravity
    if contained {
        cmd.env("AIDA_CONTAINED", "1");
        // trace:TASK-2631 | ai:codex — do not inherit a native parent's display marker.
        cmd.env("AIDA_CONTAINMENT_POSTURE", "aida");
    } else {
        cmd.env_remove("AIDA_CONTAINED");
        // trace:TASK-2631 | ai:codex
        cmd.env("AIDA_CONTAINMENT_POSTURE", "none");
    }
    // Consume prior-attempt markers only after resume preparation succeeds.
    // trace:BUG-2488 | ai:codex
    if let Err(error) = prepare_resume_attempt_signals(host_main, cwd, seat, &mut cmd) {
        tee.stop();
        revoke_headless_child_grant(&child_grant.id);
        return Err(error);
    }
    // A watch-launched reviewer is recorded before the wait. trace:TASK-1993 | ai:claude
    let status = reviewer_status_with_handle_guard(cmd.stdout(Stdio::from(log)), &mut handle_guard)
        .context("failed to spawn claude")?;
    tee.stop();
    // trace:BUG-B6B1278890774EF993D2EBC327862470-151 | ai:claude
    record_headless_session_end(cwd, session_id);
    Ok((status, child_grant.id))
}

// trace:BUG-2488 | ai:codex
pub(crate) fn prepare_resume_attempt_signals(
    host_main: Option<&Path>,
    cwd: &Path,
    seat: &str,
    cmd: &mut std::process::Command,
) -> Result<()> {
    if crate::canonical_role_name(seat) != "implementer" {
        return Ok(());
    }
    let main = host_main
        .map(Path::to_path_buf)
        .unwrap_or_else(|| crate::main_worktree_root_from(cwd));
    let canonical = cwd.canonicalize()?;
    if let Some(lease) = crate::list_leases(&main)
        .into_iter()
        .find(|lease| lease.worktree_path.canonicalize().ok().as_ref() == Some(&canonical))
    {
        let (punt, hold) =
            crate::standalone_implementer::provision_attempt_signals(&main, cwd, &lease.scope)?;
        cmd.env(crate::punt::SIGNAL_FILE_ENV, punt)
            .env(crate::punt::HOLD_SIGNAL_FILE_ENV, hold);
    }
    Ok(())
}

/// BUG-2386: what a headless resume continues — the vendor that recorded the
/// session and the id that vendor resumes it by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HeadlessResume<'a> {
    pub(crate) vendor: HeadlessVendor,
    /// The AIDA session id (Claude's `--session-id`; the handle key).
    pub(crate) session_id: &'a str,
    /// The id the vendor resumes by: the same id for Claude, the bound
    /// thread id for Codex.
    pub(crate) resume_id: &'a str,
}

/// BUG-2386: the vendor-neutral [`spawn_claude_headless_resume_granted`]:
/// Claude keeps its own path byte for byte; Codex resumes its thread with
/// `codex exec resume <thread> <prompt>`, on the posture, grant and identity
/// env a fresh headless Codex child gets. Agy is refused by name.
// trace:BUG-2386 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_vendor_headless_resume_granted(
    host_main: Option<&Path>,
    target: &HeadlessResume<'_>,
    prompt: &str,
    log_path: &Path,
    cwd: &Path,
    tee_opts: &crate::headless_tee::TeeOptions,
    contained: bool,
    seat: &str,
) -> Result<(std::process::ExitStatus, String)> {
    use std::process::{Command, Stdio};
    let vendor = target.vendor;
    match vendor {
        HeadlessVendor::Claude => {
            return spawn_claude_headless_resume_granted(
                host_main,
                prompt,
                target.session_id,
                log_path,
                cwd,
                tee_opts,
                contained,
                seat,
            )
        }
        HeadlessVendor::Agy => anyhow::bail!("{AGY_RESUME_REFUSAL}"),
        HeadlessVendor::Codex => {}
    }
    let contained = aida_contained_env()? || contained;
    // Refuse before the log file and tee exist: a contained parent never
    // resumes its child uncontained. trace:BUG-2144 | ai:claude
    require_vendor_containment(vendor, contained, &format!("resumed {seat} session"))?;
    let posture = containment_posture_in(cwd, vendor.as_str(), contained)?;
    let agent_seat = match crate::canonical_role_name(seat).as_str() {
        "reviewer" => aida_core::agents_config::AgentSeat::Reviewer,
        "advisor" => aida_core::agents_config::AgentSeat::Advisor,
        _ => aida_core::agents_config::AgentSeat::Implementer,
    };
    let tuning = resolve_agent_tuning(cwd, vendor, agent_seat);
    // trace:TASK-2902 | ai:codex
    if vendor == HeadlessVendor::Codex && crate::canonical_role_name(seat) == "implementer" {
        crate::native_publication::prepare(
            &host_main
                .map(Path::to_path_buf)
                .unwrap_or_else(|| crate::main_worktree_root_from(cwd)),
            cwd,
        )?;
    }
    // Unleased resumes remain supported; only leased implementers receive the
    // seat-local handshake. Resolve aliases before comparing runtime paths.
    // trace:BUG-2488 | ai:codex
    let signal_scope = if crate::canonical_role_name(seat) == "implementer" {
        let main = host_main
            .map(Path::to_path_buf)
            .unwrap_or_else(|| crate::main_worktree_root_from(cwd));
        let canonical_cwd = cwd.canonicalize()?;
        crate::list_leases(&main)
            .into_iter()
            .find(|l| l.worktree_path.canonicalize().ok().as_ref() == Some(&canonical_cwd))
            .map(|l| l.scope)
    } else {
        None
    };
    let effective_prompt = codex_inline_prompt(prompt).unwrap_or_else(|| prompt.to_string());
    if let Some(dir) = log_path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
    }
    let log = std::fs::File::create(log_path)
        .with_context(|| format!("failed to create headless log {}", log_path.display()))?;
    let tee = crate::headless_tee::start_tee(log_path, tee_opts);
    let (ceiling_key, ceiling_value) = crate::bg_wait_ceiling_env(Some(cwd));
    // trace:TASK-2708 | ai:claude — hoisted so the shared argv builder stays
    // beside the headless env stamp below.
    let sandboxed_grant = sandboxed_child_grant(vendor, contained)?;
    // Same order as the Claude resume path: the argv comes from the shared
    // headless builder right beside the launch (BUG-342 ratchet).
    // trace:BUG-2386 | ai:claude
    let (program, args) = os_wrapped_program_and_args(
        host_main,
        cwd,
        &resolve_agent_program(vendor.program()),
        codex_resume_exec_args(
            {
                let mut args = headless_vendor_args_contained_in(
                    host_main,
                    vendor,
                    &effective_prompt,
                    target.session_id,
                    contained.then_some(cwd),
                    tuning.model.as_deref(),
                    tuning.effort.as_deref(),
                )?;
                // TASK-2712 F2/F8: the resumed lane's own tree and seat role.
                // trace:TASK-2712 TASK-2032 | ai:claude
                rescope_native_codex_for_seat(host_main, vendor, &mut args, cwd, seat)?;
                args
            },
            target.resume_id,
        )?,
    )?;
    let child_grant = crate::launch::issue_launch_child(
        cwd,
        seat,
        &crate::current_user_id(None),
        sandboxed_grant,
    )?;
    let mut cmd = Command::new(program);
    cmd.current_dir(cwd)
        .args(args)
        .env("AIDA_HEADLESS", "1")
        .env("AIDA_SESSION_ROLE", seat)
        .env(crate::gate::seat_authority::GRANT_ENV, &child_grant.id)
        .env("AIDA_DRIVE_ROOT", headless_worktree_root())
        .env("PATH", drive_path_env().unwrap_or_default())
        .envs(crate::headless_wait_envs(ceiling_key, &ceiling_value));
    // trace:BUG-2488 | ai:codex — an unleased resume must not inherit a
    // launcher-only path outside its writable tree.
    cmd.env_remove(crate::punt::SIGNAL_FILE_ENV)
        .env_remove(crate::punt::HOLD_SIGNAL_FILE_ENV);
    if let Some(scope) = signal_scope.as_deref() {
        crate::standalone_implementer::apply_seat_signals(&mut cmd, cwd, scope);
    }
    let mut handle_guard = child_reservation_guard();
    apply_resumed_child_agent_identity(&mut cmd, vendor, seat, target.session_id);
    apply_headless_harness_version(host_main, &mut cmd, vendor, cwd);
    scrub_launcher_session_env(&mut cmd);
    // Codex never runs AIDA-contained; its native sandbox is the posture.
    cmd.env_remove("AIDA_CONTAINED");
    stamp_containment_posture(&mut cmd, posture);
    // Finalize like every fresh headless Codex launch; a refusal leaves no
    // live child grant. trace:BUG-2386 | ai:claude
    cmd.env("AIDA_USER", crate::current_user_id(None))
        .env("AIDA_PROJECT_ROOT", headless_worktree_root());
    if let Err(error) = crate::vendors::profile::finalize_codex(&mut cmd) {
        tee.stop();
        revoke_headless_child_grant(&child_grant.id);
        return Err(error);
    }
    // A refused resume must retain the persisted hold so admission can retry.
    // trace:BUG-2488 | ai:codex
    if let Some(scope) = signal_scope.as_deref() {
        let main = host_main
            .map(Path::to_path_buf)
            .unwrap_or_else(|| crate::main_worktree_root_from(cwd));
        if let Err(error) =
            crate::standalone_implementer::provision_attempt_signals(&main, cwd, scope)
        {
            tee.stop();
            revoke_headless_child_grant(&child_grant.id);
            return Err(error);
        }
    }
    let status = reviewer_status_with_handle_guard(cmd.stdout(Stdio::from(log)), &mut handle_guard)
        .context("failed to spawn codex")?;
    tee.stop();
    record_headless_session_end(cwd, target.session_id);
    Ok((status, child_grant.id))
}

/// BUG-2386: [`spawn_vendor_headless_resume_granted`] without the grant id.
// trace:BUG-2386 | ai:claude
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_vendor_headless_resume(
    host_main: Option<&Path>,
    target: &HeadlessResume<'_>,
    prompt: &str,
    log_path: &Path,
    cwd: &Path,
    tee_opts: &crate::headless_tee::TeeOptions,
    contained: bool,
    seat: &str,
) -> Result<std::process::ExitStatus> {
    spawn_vendor_headless_resume_granted(
        host_main, target, prompt, log_path, cwd, tee_opts, contained, seat,
    )
    .map(|(status, _)| status)
}

/// BUG-2386: the vendor and resumable id of the AIDA session `session_id`
/// that ran on `vendor` in `worktree`: Claude resumes by the same id; Codex
/// by the thread bound to the session (lease stamp, headless log, or the one
/// rollout started in `worktree` since `since`).
// trace:BUG-2386 | ai:claude
pub(crate) fn resolve_headless_resume_id(
    vendor: HeadlessVendor,
    project_root: &Path,
    session_id: &str,
    lease_id: Option<&str>,
    worktree: &Path,
    since: Option<SystemTime>,
) -> Result<String> {
    match vendor {
        HeadlessVendor::Claude => Ok(session_id.to_string()),
        HeadlessVendor::Codex => {
            crate::codex_resume::resolve_thread_id(&crate::codex_resume::ThreadLookup {
                project_root,
                session_id,
                lease_id,
                worktree,
                since,
            })
        }
        HeadlessVendor::Agy => anyhow::bail!("{AGY_RESUME_REFUSAL}"),
    }
}
