//! Terminal focus/send adapters for live AIDA sessions.
//!
//! `aida session focus` and `aida session send` deliberately act only through
//! terminal/multiplexer APIs. They never write to a tty device.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use colored::Colorize;
use serde::Serialize;

use crate::agent_registry::{self, AgentClassifyContext, AgentRegistryView, TerminalIdentity};
use crate::cli::OutputFormat;

const TERMINATOR_BUS: &str = "net.aida.Terminator";
const TERMINATOR_PATH: &str = "/net/aida/Terminator";
const TERMINATOR_IFACE: &str = "net.aida.Terminator";
const TERMINATOR_TEMPLATE_KEY: &str = "terminal/terminator/aida_terminator.py";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum TerminalAdapterKind {
    Tmux,
    Wezterm,
    Terminator,
    Null,
}

impl TerminalAdapterKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Tmux => "tmux",
            Self::Wezterm => "wezterm",
            Self::Terminator => "terminator",
            Self::Null => "null",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum TerminalAction {
    Focus,
    Send,
    Mail,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct TerminalCommandReport {
    pub(crate) target: String,
    pub(crate) resolved_session: Option<String>,
    pub(crate) resolved_agent: Option<String>,
    pub(crate) resolved_spec: Option<String>,
    pub(crate) emulator: Option<String>,
    pub(crate) adapter: TerminalAdapterKind,
    pub(crate) action: TerminalAction,
    pub(crate) taken: bool,
    pub(crate) hint: Option<String>,
}

#[derive(Debug, Clone)]
struct ResolvedSession {
    view: AgentRegistryView,
    terminal: Option<TerminalIdentity>,
}

trait TerminalAdapter {
    fn kind(&self) -> TerminalAdapterKind;
    fn focus(&self) -> AdapterOutcome;
    fn send(&self, text: &str) -> AdapterOutcome;
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AdapterOutcome {
    taken: bool,
    hint: Option<String>,
}

impl AdapterOutcome {
    fn taken() -> Self {
        Self {
            taken: true,
            hint: None,
        }
    }

    fn refused(hint: impl Into<String>) -> Self {
        Self {
            taken: false,
            hint: Some(hint.into()),
        }
    }
}

struct TmuxAdapter {
    pane: String,
    socket: Option<String>,
}

struct WeztermAdapter {
    pane: String,
}

struct TerminatorAdapter {
    uuid: String,
    send_token: Option<String>,
}

struct NullAdapter {
    hint: String,
}

impl TerminalAdapter for TmuxAdapter {
    fn kind(&self) -> TerminalAdapterKind {
        TerminalAdapterKind::Tmux
    }

    fn focus(&self) -> AdapterOutcome {
        let (program, args) = tmux_focus_command(&self.pane, self.socket.as_deref());
        run_command(&program, &args, "tmux focus")
    }

    fn send(&self, text: &str) -> AdapterOutcome {
        let (program, args) = tmux_send_command(&self.pane, self.socket.as_deref(), text);
        run_command(&program, &args, "tmux send")
    }
}

impl TerminalAdapter for WeztermAdapter {
    fn kind(&self) -> TerminalAdapterKind {
        TerminalAdapterKind::Wezterm
    }

    fn focus(&self) -> AdapterOutcome {
        let (program, args) = wezterm_focus_command(&self.pane);
        run_command(&program, &args, "wezterm focus")
    }

    fn send(&self, text: &str) -> AdapterOutcome {
        let (program, args) = wezterm_send_command(&self.pane, text);
        run_command(&program, &args, "wezterm send")
    }
}

impl TerminalAdapter for TerminatorAdapter {
    fn kind(&self) -> TerminalAdapterKind {
        TerminalAdapterKind::Terminator
    }

    fn focus(&self) -> AdapterOutcome {
        let (program, args) = terminator_focus_command(&self.uuid);
        run_terminator_command(&program, &args, "Terminator plugin focus")
    }

    fn send(&self, text: &str) -> AdapterOutcome {
        let (program, args) = match self.send_token.as_deref() {
            Some(token) => terminator_send_token_command(&self.uuid, text, token),
            None => terminator_send_command(&self.uuid, text),
        };
        run_terminator_command(&program, &args, "Terminator plugin send")
    }
}

impl TerminalAdapter for NullAdapter {
    fn kind(&self) -> TerminalAdapterKind {
        TerminalAdapterKind::Null
    }

    fn focus(&self) -> AdapterOutcome {
        AdapterOutcome::refused(self.hint.clone())
    }

    fn send(&self, _text: &str) -> AdapterOutcome {
        AdapterOutcome::refused(format!(
            "{}; use `aida mailbox send` for non-injective delivery",
            self.hint
        ))
    }
}

// trace:STORY-995 | ai:codex
pub(crate) fn focus_session(target: &str) -> Result<()> {
    let project_root = crate::find_project_root()?;
    let resolved = resolve_session_target(&project_root, target)?;
    let adapter = adapter_for(
        resolved.terminal.as_ref(),
        locate_hint(&resolved.view),
        terminal_send_token(&project_root),
    );
    let outcome = adapter.focus();
    let report = report_for(
        target,
        &resolved,
        adapter.kind(),
        TerminalAction::Focus,
        outcome,
    );
    print_report(&report, false)
}

// trace:STORY-995 | ai:codex
pub(crate) fn send_session(target: &str, text: &str, enter: bool, mail: bool) -> Result<()> {
    let project_root = crate::find_project_root()?;
    let resolved = resolve_session_target(&project_root, target)?;
    let adapter = adapter_for(
        resolved.terminal.as_ref(),
        locate_hint(&resolved.view),
        terminal_send_token(&project_root),
    );
    let action = if mail {
        send_mail_notice(&project_root, &resolved.view, text)?;
        let outcome = if adapter.kind() == TerminalAdapterKind::Null {
            AdapterOutcome::refused(format!(
                "{}; mailbox notice delivered",
                locate_hint(&resolved.view)
            ))
        } else {
            adapter.send("\n")
        };
        report_for(
            target,
            &resolved,
            adapter.kind(),
            TerminalAction::Mail,
            outcome,
        )
    } else {
        let body = if enter {
            format!("{text}\n")
        } else {
            text.to_string()
        };
        let outcome = adapter.send(&body);
        report_for(
            target,
            &resolved,
            adapter.kind(),
            TerminalAction::Send,
            outcome,
        )
    };
    print_report(&action, !mail)
}

// trace:STORY-995 | ai:codex
pub(crate) fn install_terminal(target: &str) -> Result<()> {
    match target.trim().to_ascii_lowercase().as_str() {
        "terminator" => install_terminator_plugin(),
        other => anyhow::bail!("unknown terminal helper '{other}'; expected terminator"),
    }
}

fn resolve_session_target(project_root: &Path, target: &str) -> Result<ResolvedSession> {
    let target = target.trim();
    if target.is_empty() {
        anyhow::bail!("session target cannot be empty");
    }
    let cfg = agent_registry::Config::load(project_root);
    let ctx = AgentClassifyContext::new(chrono::Utc::now(), cfg.busy_threshold_secs, Vec::new());
    let mut matches: Vec<AgentRegistryView> = agent_registry::list_agent_views(project_root, &ctx)
        .into_iter()
        .filter(|view| session_matches(view, target))
        .collect();
    matches.sort_by(|a, b| {
        b.last_active_at
            .cmp(&a.last_active_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    match matches.len() {
        0 => anyhow::bail!("no registered session found matching '{}'", target),
        1 => {
            let view = matches.remove(0);
            let terminal = view.terminal.clone();
            Ok(ResolvedSession { view, terminal })
        }
        _ => {
            let names: Vec<String> = matches.iter().map(display_session_match).collect();
            anyhow::bail!(
                "session target '{}' is ambiguous — matches: {}",
                target,
                names.join(", ")
            )
        }
    }
}

fn session_matches(view: &AgentRegistryView, target: &str) -> bool {
    let t = target.to_ascii_lowercase();
    let prefix = |s: &str| s.to_ascii_lowercase().starts_with(&t);
    prefix(&view.id)
        || view
            .native_session_id
            .as_deref()
            .map(prefix)
            .unwrap_or(false)
        || view
            .name
            .as_deref()
            .map(|s| s.eq_ignore_ascii_case(target))
            .unwrap_or(false)
        || view
            .current_spec
            .as_deref()
            .map(|s| s.eq_ignore_ascii_case(target))
            .unwrap_or(false)
}

fn display_session_match(view: &AgentRegistryView) -> String {
    view.name.clone().unwrap_or_else(|| {
        view.native_session_id
            .clone()
            .unwrap_or_else(|| view.id.clone())
            .chars()
            .take(8)
            .collect()
    })
}

fn adapter_for(
    terminal: Option<&TerminalIdentity>,
    fallback_hint: String,
    send_token: Option<String>,
) -> Box<dyn TerminalAdapter> {
    let Some(terminal) = terminal else {
        return Box::new(NullAdapter {
            hint: fallback_hint,
        });
    };
    if let Some(pane) = terminal.tmux_pane.as_deref().filter(|s| !s.is_empty()) {
        return Box::new(TmuxAdapter {
            pane: pane.to_string(),
            socket: terminal.tmux_socket.clone(),
        });
    }
    if let Some(pane) = terminal.wezterm_pane.as_deref().filter(|s| !s.is_empty()) {
        return Box::new(WeztermAdapter {
            pane: pane.to_string(),
        });
    }
    if let Some(uuid) = terminal
        .terminator_uuid
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        return Box::new(TerminatorAdapter {
            uuid: uuid.to_string(),
            send_token,
        });
    }
    Box::new(NullAdapter {
        hint: fallback_hint,
    })
}

fn locate_hint(view: &AgentRegistryView) -> String {
    let terminal = agent_registry::terminal_cell(view.terminal.as_ref(), view.tty.as_deref());
    let title = agent_registry::launch_title(
        view.role.as_deref().unwrap_or("agent"),
        view.current_spec.as_deref(),
        view.native_session_id.as_deref().unwrap_or(&view.id),
    );
    format!("locate manually: on {terminal}, tab titled '{title}'")
}

fn report_for(
    target: &str,
    resolved: &ResolvedSession,
    adapter: TerminalAdapterKind,
    action: TerminalAction,
    outcome: AdapterOutcome,
) -> TerminalCommandReport {
    TerminalCommandReport {
        target: target.to_string(),
        resolved_session: resolved
            .view
            .native_session_id
            .clone()
            .or(Some(resolved.view.id.clone())),
        resolved_agent: resolved
            .view
            .name
            .clone()
            .or(Some(resolved.view.agent_type.clone())),
        resolved_spec: resolved.view.current_spec.clone(),
        emulator: resolved
            .terminal
            .as_ref()
            .and_then(|t| t.emulator.clone())
            .or_else(|| Some(adapter.as_str().to_string()).filter(|s| s != "null")),
        adapter,
        action,
        taken: outcome.taken,
        hint: outcome.hint,
    }
}

fn print_report(report: &TerminalCommandReport, send_refusal_can_fail: bool) -> Result<()> {
    if matches!(crate::output_format_override(), Some(OutputFormat::Json)) {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }
    if report.taken {
        println!(
            "{} {} via {}",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            match report.action {
                TerminalAction::Focus => "focused session",
                TerminalAction::Send => "sent text",
                TerminalAction::Mail => "sent mailbox notice and nudged session",
            },
            report.adapter.as_str().cyan()
        );
        return Ok(());
    }
    let hint = report
        .hint
        .as_deref()
        .unwrap_or("no actionable terminal adapter");
    if send_refusal_can_fail && matches!(report.action, TerminalAction::Send) {
        anyhow::bail!("{hint}");
    }
    eprintln!("{} {hint}", "warning:".yellow());
    Ok(())
}

fn run_command(program: &str, args: &[String], label: &str) -> AdapterOutcome {
    match Command::new(program).args(args).status() {
        Ok(status) if status.success() => AdapterOutcome::taken(),
        Ok(status) => AdapterOutcome::refused(format!("{label} failed with status {status}")),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            AdapterOutcome::refused(format!("`{program}` not on PATH"))
        }
        Err(err) => AdapterOutcome::refused(format!("{label} failed: {err}")),
    }
}

fn run_terminator_command(program: &str, args: &[String], label: &str) -> AdapterOutcome {
    match Command::new(program).args(args).output() {
        Ok(out) if out.status.success() && String::from_utf8_lossy(&out.stdout).contains("true") => {
            AdapterOutcome::taken()
        }
        Ok(out) if out.status.success() => AdapterOutcome::refused(
            "Terminator plugin returned false; run `aida terminal install terminator`, enable the plugin, then restart Terminator",
        ),
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let hint = if stderr.contains("was not provided")
                || stderr.contains("ServiceUnknown")
                || stderr.contains("not found")
                || stderr.contains("No such")
            {
                "Terminator plugin not installed or not enabled; run `aida terminal install terminator`, enable it in Preferences > Plugins, then restart Terminator".to_string()
            } else {
                format!("{label} failed: {}", stderr.trim())
            };
            AdapterOutcome::refused(hint)
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => AdapterOutcome::refused(
            "`busctl` not on PATH; install systemd busctl or use tmux/wezterm for native focus/send",
        ),
        Err(err) => AdapterOutcome::refused(format!("{label} failed: {err}")),
    }
}

pub(crate) fn tmux_focus_command(pane: &str, socket: Option<&str>) -> (String, Vec<String>) {
    let mut args = Vec::new();
    if let Some(socket) = socket.filter(|s| !s.is_empty()) {
        args.extend(["-S".to_string(), socket.to_string()]);
    }
    args.extend([
        "select-window".to_string(),
        "-t".to_string(),
        pane.to_string(),
        ";".to_string(),
        "select-pane".to_string(),
        "-t".to_string(),
        pane.to_string(),
    ]);
    ("tmux".to_string(), args)
}

pub(crate) fn tmux_send_command(
    pane: &str,
    socket: Option<&str>,
    text: &str,
) -> (String, Vec<String>) {
    let mut args = Vec::new();
    if let Some(socket) = socket.filter(|s| !s.is_empty()) {
        args.extend(["-S".to_string(), socket.to_string()]);
    }
    args.extend([
        "send-keys".to_string(),
        "-t".to_string(),
        pane.to_string(),
        "-l".to_string(),
        text.to_string(),
    ]);
    ("tmux".to_string(), args)
}

pub(crate) fn wezterm_focus_command(pane: &str) -> (String, Vec<String>) {
    (
        "wezterm".to_string(),
        vec![
            "cli".to_string(),
            "activate-pane".to_string(),
            "--pane-id".to_string(),
            pane.to_string(),
        ],
    )
}

pub(crate) fn wezterm_send_command(pane: &str, text: &str) -> (String, Vec<String>) {
    (
        "wezterm".to_string(),
        vec![
            "cli".to_string(),
            "send-text".to_string(),
            "--pane-id".to_string(),
            pane.to_string(),
            "--no-paste".to_string(),
            text.to_string(),
        ],
    )
}

pub(crate) fn terminator_focus_command(uuid: &str) -> (String, Vec<String>) {
    terminator_call_command("Focus", &["s", uuid])
}

pub(crate) fn terminator_send_command(uuid: &str, text: &str) -> (String, Vec<String>) {
    terminator_call_command("Send", &["s", uuid, "s", text])
}

pub(crate) fn terminator_send_token_command(
    uuid: &str,
    text: &str,
    token: &str,
) -> (String, Vec<String>) {
    terminator_call_command("SendToken", &["s", uuid, "s", text, "s", token])
}

fn terminator_call_command(method: &str, typed_args: &[&str]) -> (String, Vec<String>) {
    let mut args = vec![
        "--user".to_string(),
        "call".to_string(),
        TERMINATOR_BUS.to_string(),
        TERMINATOR_PATH.to_string(),
        TERMINATOR_IFACE.to_string(),
        method.to_string(),
    ];
    args.extend(typed_args.iter().map(|s| s.to_string()));
    ("busctl".to_string(), args)
}

fn send_mail_notice(project_root: &Path, view: &AgentRegistryView, text: &str) -> Result<()> {
    let recipient = view
        .name
        .clone()
        .or_else(|| view.role.clone())
        .unwrap_or_else(|| view.agent_type.clone());
    let msg = aida_core::mailbox::Message {
        id: uuid::Uuid::new_v4().to_string(),
        thread_id: uuid::Uuid::new_v4().to_string(),
        from: crate::current_user_id(None),
        to: aida_core::mailbox::Recipient::Agent(recipient),
        timestamp: chrono::Utc::now().timestamp_millis(),
        in_reply_to: None,
        body: text.to_string(),
        urgent: false,
        intent: aida_core::mailbox::Intent::Fyi,
        retracted: false,
        deleted: false,
    };
    crate::mailbox_store::write_message(project_root, &msg)
}

fn terminal_send_token(project_root: &Path) -> Option<String> {
    let path = project_root.join(".aida").join("config.toml");
    let body = std::fs::read_to_string(path).ok()?;
    let value = body.parse::<toml::Value>().ok()?;
    value
        .get("terminal")
        .and_then(|v| v.get("send_token"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn install_terminator_plugin() -> Result<()> {
    let body = aida_core::templates::EMBEDDED_TEMPLATES
        .get(TERMINATOR_TEMPLATE_KEY)
        .ok_or_else(|| anyhow::anyhow!("embedded Terminator plugin template is missing"))?;
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("could not determine home dir"))?;
    let dir = home.join(".config").join("terminator").join("plugins");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let dest = dir.join("aida_terminator.py");
    let changed = write_if_changed(&dest, body.as_bytes())?;
    if changed {
        println!(
            "{} installed Terminator plugin at {}",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            dest.display()
        );
    } else {
        println!(
            "{} Terminator plugin already current at {}",
            crate::glyph(crate::glyphs::Glyph::Check).green(),
            dest.display()
        );
    }
    println!("  Enable it in Terminator Preferences > Plugins, then restart Terminator.");
    println!("  Trust note: Send injects keystrokes into a shell through the user session bus.");
    Ok(())
}

fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<bool> {
    match std::fs::read(path) {
        Ok(existing) if existing == bytes => return Ok(false),
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
    }
    aida_core::write_atomic(path, bytes).with_context(|| format!("writing {}", path.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term(emulator: &str) -> TerminalIdentity {
        let mut t = TerminalIdentity::default();
        t.emulator = Some(emulator.to_string());
        t
    }

    #[test]
    fn adapter_selection_prefers_supported_terminal_ids() {
        let mut tmux = term("tmux");
        tmux.tmux_pane = Some("%7".to_string());
        tmux.tmux_socket = Some("/tmp/tmux.sock".to_string());
        assert_eq!(
            adapter_for(Some(&tmux), "hint".to_string(), None).kind(),
            TerminalAdapterKind::Tmux
        );

        let mut wez = term("wezterm");
        wez.wezterm_pane = Some("12".to_string());
        assert_eq!(
            adapter_for(Some(&wez), "hint".to_string(), None).kind(),
            TerminalAdapterKind::Wezterm
        );

        let mut terminator = term("terminator");
        terminator.terminator_uuid = Some("term-1".to_string());
        assert_eq!(
            adapter_for(Some(&terminator), "hint".to_string(), None).kind(),
            TerminalAdapterKind::Terminator
        );
    }

    #[test]
    fn null_adapter_refuses_with_hint() {
        let adapter = adapter_for(None, "on pts/0".to_string(), None);
        assert_eq!(adapter.kind(), TerminalAdapterKind::Null);
        assert_eq!(adapter.focus().hint.as_deref(), Some("on pts/0"));
        assert!(adapter
            .send("hello")
            .hint
            .as_deref()
            .unwrap()
            .contains("aida mailbox send"));
    }

    #[test]
    fn tmux_command_lines_are_stable() {
        assert_eq!(
            tmux_focus_command("%3", Some("/tmp/tmux-1000/default")),
            (
                "tmux".to_string(),
                vec![
                    "-S",
                    "/tmp/tmux-1000/default",
                    "select-window",
                    "-t",
                    "%3",
                    ";",
                    "select-pane",
                    "-t",
                    "%3"
                ]
                .into_iter()
                .map(String::from)
                .collect()
            )
        );
        assert_eq!(
            tmux_send_command("%3", None, "hi\n").1,
            vec!["send-keys", "-t", "%3", "-l", "hi\n"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn wezterm_command_lines_are_stable() {
        assert_eq!(
            wezterm_focus_command("44").1,
            vec!["cli", "activate-pane", "--pane-id", "44"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            wezterm_send_command("44", "hi").1,
            vec!["cli", "send-text", "--pane-id", "44", "--no-paste", "hi"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn terminator_uses_busctl_session_bus() {
        let args = terminator_send_command("uuid-1", "hi").1;
        assert!(args.starts_with(&[
            "--user".to_string(),
            "call".to_string(),
            TERMINATOR_BUS.to_string(),
            TERMINATOR_PATH.to_string(),
            TERMINATOR_IFACE.to_string(),
            "Send".to_string(),
        ]));
        assert_eq!(args[6..], ["s", "uuid-1", "s", "hi"]);
        let token_args = terminator_send_token_command("uuid-1", "hi", "secret").1;
        assert_eq!(token_args[5], "SendToken");
        assert_eq!(token_args[6..], ["s", "uuid-1", "s", "hi", "s", "secret"]);
    }
}
