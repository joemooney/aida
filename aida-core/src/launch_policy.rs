//! Pure launch policy for agent children: the closed vendor and kind sets,
//! the AIDA environment keys a launch owns, and the posture-bearing flags a
//! caller-supplied argv may not carry. No I/O; `aida-cli-lib`'s `launch/`
//! module constructs vendor processes. The TUI shares the control-env policy
//! for its checked AIDA children.
//!
//! TASK-1749 C1 holds here: nothing in this module clears the environment.
//! The launcher strips exactly the [`OWNED_KEYS`] it does not set and lets
//! every other variable pass, until the per-vendor allowlist slice.
// trace:STORY-2326 | ai:claude

use std::fmt;

/// `AIDA_SESSION_ROLE`: the seat a child runs as.
pub const ROLE_ENV: &str = "AIDA_SESSION_ROLE";
/// The session grant variable: the child's grant selector (an explicit `""` is a
/// baseline child).
pub const GRANT_ENV: &str = "AIDA_SESSION_GRANT";
/// `AIDA_SESSION_PURPOSE`: describes the launcher's own session.
pub const PURPOSE_ENV: &str = "AIDA_SESSION_PURPOSE";
/// `AIDA_CONTAINED`: set only when the child runs inside the AIDA boundary.
pub const CONTAINED_ENV: &str = "AIDA_CONTAINED";
/// `AIDA_CONTAINMENT_POSTURE`: `aida`, `native` or `none`.
pub const POSTURE_ENV: &str = "AIDA_CONTAINMENT_POSTURE";
/// `AIDA_ROLE_INSTANCE`: the companion marker of a seat instance.
pub const ROLE_INSTANCE_ENV: &str = "AIDA_ROLE_INSTANCE";
/// `AIDA_STANDALONE_IMPLEMENTER`: binds only the child its launcher set it on.
pub const STANDALONE_MARKER_ENV: &str = "AIDA_STANDALONE_IMPLEMENTER";
/// `AIDA_HEADLESS_LANE`: a child lane gets its own lane marker.
pub const LANE_MARKER_ENV: &str = "AIDA_HEADLESS_LANE";
/// `AIDA_AGENT_REGISTRY_TOKEN`: the registry row's write token.
pub const REGISTRY_TOKEN_ENV: &str = "AIDA_AGENT_REGISTRY_TOKEN";
/// `AIDA_HANDLE_LAUNCH_TOKEN`: the handle reservation's launch token.
pub const HANDLE_LAUNCH_TOKEN_ENV: &str = "AIDA_HANDLE_LAUNCH_TOKEN";
/// `AIDA_AGENT_CMD`: the test hook that replaces the vendor program.
pub const AGENT_CMD_ENV: &str = "AIDA_AGENT_CMD";

/// What a launch does with one key it owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owned {
    /// The launch sets it (or explicitly clears it); an ambient value never
    /// reaches the child.
    Decided,
    /// The ambient value passes unchanged, unless the launch sets its own.
    Forwarded,
}

/// Every AIDA key a launch owns, with its disposition. One table, pinned by
/// tests, so the owned set cannot drift between launch paths.
///
/// The two tokens are `Forwarded` on the vendor process only. They must never
/// appear in a tool shell's environment (`codex_tool_env::TOOL_ENV`).
pub const OWNED_KEYS: &[(&str, Owned)] = &[
    (ROLE_ENV, Owned::Decided),
    (GRANT_ENV, Owned::Decided),
    (PURPOSE_ENV, Owned::Decided),
    (CONTAINED_ENV, Owned::Decided),
    (POSTURE_ENV, Owned::Decided),
    (STANDALONE_MARKER_ENV, Owned::Decided),
    (LANE_MARKER_ENV, Owned::Decided),
    (ROLE_INSTANCE_ENV, Owned::Forwarded),
    (REGISTRY_TOKEN_ENV, Owned::Forwarded),
    (HANDLE_LAUNCH_TOKEN_ENV, Owned::Forwarded),
];

/// Keys that belong on the vendor process only, never in a tool shell.
pub const VENDOR_PROCESS_ONLY_KEYS: &[&str] = &[REGISTRY_TOKEN_ENV, HANDLE_LAUNCH_TOKEN_ENV];

/// Keys a `--bg` hand-off never carries: the daemon-run session resolves its
/// seat from its lane lease, not from the dispatcher's command.
pub const BACKGROUND_FORBIDDEN_KEYS: &[&str] = &[ROLE_ENV, GRANT_ENV, PURPOSE_ENV];

/// The owned keys a launch removes from the ambient environment before it
/// sets its own.
pub fn decided_keys() -> impl Iterator<Item = &'static str> {
    OWNED_KEYS
        .iter()
        .filter(|(_, how)| *how == Owned::Decided)
        .map(|(key, _)| *key)
}

// Compatibility path: identity and descriptors have one owner.
// trace:STORY-2539 | ai:codex
pub use crate::vendors::{LaunchVendor, UnknownVendor};

/// What a launch is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LaunchKind {
    /// A new interactive or headless session.
    Fresh,
    /// A resume of a recorded session.
    Resume,
    /// This process becomes the vendor (`exec`).
    Exec,
    /// A `--bg` hand-off to the vendor's own daemon.
    Background,
    /// A vendor probe (version, agents, MCP list): no seat, never a session.
    Query,
    /// An `aida` child that continues this process's own seat.
    Control,
}

impl LaunchKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Resume => "resume",
            Self::Exec => "exec",
            Self::Background => "background",
            Self::Query => "query",
            Self::Control => "control",
        }
    }

    /// Parse a kind name exactly; an unknown name is `None`.
    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw.trim().to_ascii_lowercase().as_str() {
            "fresh" => Self::Fresh,
            "resume" => Self::Resume,
            "exec" => Self::Exec,
            "background" | "bg" => Self::Background,
            "query" => Self::Query,
            "control" => Self::Control,
            _ => return None,
        })
    }
}

/// A posture-bearing flag in caller-supplied argv. Posture comes from the
/// vendor profile, never from a request tail or `default_flags`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostureFlag(pub String);

impl fmt::Display for PostureFlag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "`{}` sets the agent's sandbox or permission posture, which AIDA decides; \
             remove it from the launch arguments",
            self.0
        )
    }
}

impl std::error::Error for PostureFlag {}

/// Whether one argument (with the next one, for `-c <value>`) sets posture.
fn posture_flag_at(args: &[String], at: usize) -> Option<String> {
    let arg = args[at].as_str();
    let name = arg.split_once('=').map_or(arg, |(name, _)| name);
    // Claude: --settings, --permission-mode, --add-dir, --dangerously-*.
    // Codex: --sandbox/-s, --ask-for-approval/-a, --full-auto, --yolo,
    // --dangerously-bypass-approvals-and-sandbox, and --profile (a profile
    // can set sandbox_mode and approval_policy). trace:STORY-2326 | ai:claude
    let named = matches!(
        name,
        "--settings"
            | "--permission-mode"
            | "--sandbox"
            | "-s"
            | "--ask-for-approval"
            | "-a"
            | "--add-dir"
            | "--full-auto"
            | "--yolo"
            | "--profile"
    ) || name.starts_with("--dangerously-");
    if named {
        return Some(arg.to_string());
    }
    // `-c <key>=…` / `--config <key>=…` and the joined `-c<key>=…` spelling,
    // where the (possibly quoted) TOML key sets the sandbox, the approval
    // policy (`approval_policy=never` is `-a never`) or selects a profile.
    let config_value = match name {
        "-c" | "--config" => match arg.split_once('=') {
            Some((_, value)) => Some(value),
            None => args.get(at + 1).map(String::as_str),
        },
        _ => arg
            .strip_prefix("-c")
            .filter(|v| !v.is_empty() && !arg.starts_with("--")),
    };
    config_value
        .filter(|value| {
            let key = value.trim_start().trim_start_matches(['"', '\'']);
            POSTURE_CONFIG_KEYS.iter().any(|k| key.starts_with(k))
        })
        .map(|value| format!("{name} {value}"))
}

/// Codex config key prefixes that set posture: the sandbox (`sandbox_mode`,
/// `sandbox_workspace_write.*`), the approval policy, and `profile` /
/// `profiles.*` (a profile can set either).
const POSTURE_CONFIG_KEYS: &[&str] = &["sandbox", "approval_policy", "profile"];

/// Refuse caller-supplied argv (a request tail or `default_flags`) that
/// carries a posture-bearing flag.
pub fn refuse_posture_flags(args: &[String]) -> Result<(), PostureFlag> {
    for (at, arg) in args.iter().enumerate() {
        if arg == "--" {
            // Everything after `--` is a positional (a prompt), not a flag.
            return Ok(());
        }
        if let Some(flag) = posture_flag_at(args, at) {
            return Err(PostureFlag(flag));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn vendor_parse_is_closed() {
        assert_eq!(LaunchVendor::parse("Claude"), Ok(LaunchVendor::Claude));
        assert_eq!(LaunchVendor::parse(" codex "), Ok(LaunchVendor::Codex));
        assert_eq!(LaunchVendor::parse("antigravity"), Ok(LaunchVendor::Agy));
        assert_eq!(LaunchVendor::parse("agy"), Ok(LaunchVendor::Agy));
        for refused in ["opencode", "gemini", "", "claude-code", "bash"] {
            assert!(LaunchVendor::parse(refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn kind_parse_is_closed() {
        for kind in [
            LaunchKind::Fresh,
            LaunchKind::Resume,
            LaunchKind::Exec,
            LaunchKind::Background,
            LaunchKind::Query,
            LaunchKind::Control,
        ] {
            assert_eq!(LaunchKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(LaunchKind::parse("daemon"), None);
    }

    #[test]
    fn owned_set_is_pinned() {
        let keys: Vec<&str> = OWNED_KEYS.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            keys,
            [
                "AIDA_SESSION_ROLE",
                "AIDA_SESSION_GRANT",
                "AIDA_SESSION_PURPOSE",
                "AIDA_CONTAINED",
                "AIDA_CONTAINMENT_POSTURE",
                "AIDA_STANDALONE_IMPLEMENTER",
                "AIDA_HEADLESS_LANE",
                "AIDA_ROLE_INSTANCE",
                "AIDA_AGENT_REGISTRY_TOKEN",
                "AIDA_HANDLE_LAUNCH_TOKEN",
            ]
        );
        // The companion marker and the two tokens are forwarded, never stripped.
        for forwarded in [
            ROLE_INSTANCE_ENV,
            REGISTRY_TOKEN_ENV,
            HANDLE_LAUNCH_TOKEN_ENV,
        ] {
            assert!(decided_keys().all(|k| k != forwarded), "{forwarded}");
        }
        for key in BACKGROUND_FORBIDDEN_KEYS {
            assert!(decided_keys().any(|k| k == *key), "{key}");
        }
        for key in VENDOR_PROCESS_ONLY_KEYS {
            assert!(keys.contains(key), "{key}");
        }
    }

    #[test]
    fn posture_flags_refused_in_caller_argv() {
        let refused: &[&[&str]] = &[
            &["--settings", "s.json"],
            &["--settings=s.json"],
            &["--permission-mode", "bypassPermissions"],
            &["--permission-mode=plan"],
            &["--dangerously-skip-permissions"],
            &["--dangerously-bypass-approvals-and-sandbox"],
            &["--sandbox", "danger-full-access"],
            &["--add-dir", "/"],
            &["--full-auto"],
            &["--yolo"],
            &["-s", "danger-full-access"],
            &["-s=workspace-write"],
            &["--ask-for-approval", "never"],
            &["--ask-for-approval=never"],
            &["-a", "never"],
            &["-c", "sandbox_mode=danger-full-access"],
            &["-c", "sandbox_workspace_write.writable_roots=[\"/\"]"],
            &["--config", "sandbox_mode=x"],
            &["--config=sandbox_mode=x"],
            &["-csandbox_mode=x"],
            &["--model", "opus", "--settings", "x"],
            // Advisor ruling 5 (round 5): the approval policy as config, a
            // quoted TOML key, and a profile (flag or config).
            &["-c", "approval_policy=never"],
            &["--config=approval_policy=\"never\""],
            &["-c", "\"sandbox_mode\"=danger-full-access"],
            &["-c", "'sandbox_mode'=danger-full-access"],
            &["--profile", "yolo"],
            &["--profile=yolo"],
            &["-c", "profile=yolo"],
            &["-c", "profiles.yolo.sandbox_mode=danger-full-access"],
        ];
        for args in refused {
            assert!(refuse_posture_flags(&argv(args)).is_err(), "{args:?}");
        }
        let allowed: &[&[&str]] = &[
            &[],
            &["--model", "opus"],
            &["-c", "model_reasoning_effort=high"],
            &["--name", "lane"],
            &["fix the --settings parser"],
            &["--", "--settings", "is only a prompt here"],
            &["-p", "prompt"],
            &["-m", "gpt-5"],
            &["--search"],
        ];
        for args in allowed {
            assert!(refuse_posture_flags(&argv(args)).is_ok(), "{args:?}");
        }
    }
}

/// An AIDA control child forwards this process's selector, never a requested
/// role. The receiving CLI validates the selector using its ordinary gate.
/// An absent selector is explicitly removed, so it cannot arrive ambiently.
/// Callers execute the kernel-held running image with `argv[0]` set to `aida`.
// trace:STORY-2326 | ai:codex
pub fn control_env(
    own_grant_selector: Option<&std::ffi::OsStr>,
) -> [(&'static str, Option<std::ffi::OsString>); 2] {
    [
        (ROLE_ENV, None),
        (
            GRANT_ENV,
            own_grant_selector.map(std::ffi::OsStr::to_os_string),
        ),
    ]
}
