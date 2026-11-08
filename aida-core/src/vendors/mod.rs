//! Pure vendor registry. Capabilities describe syntax and candidate support;
//! they never confer launch admission, containment credit or runtime success.
// trace:STORY-2539 | ai:codex
use std::fmt;

/// The existing closed launch key, re-exported by `launch_policy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LaunchVendor {
    Claude,
    Codex,
    Agy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownVendor(pub String);

impl fmt::Display for UnknownVendor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`{}` is not a vendor AIDA can launch; use ", self.0)?;
        for (index, p) in PROFILES.iter().enumerate() {
            if index > 0 {
                f.write_str(if index == PROFILES.len() - 1 {
                    " or "
                } else {
                    ", "
                })?;
            }
            f.write_str(p.launch_name)?;
        }
        Ok(())
    }
}
impl std::error::Error for UnknownVendor {}

impl LaunchVendor {
    /// Parsing a name recognizes identity only; launch policy still admits it.
    pub fn parse(raw: &str) -> Result<Self, UnknownVendor> {
        let name = raw.trim().to_ascii_lowercase();
        PROFILES
            .iter()
            .find(|p| p.aliases.contains(&name.as_str()))
            .map(|p| p.vendor)
            .ok_or_else(|| UnknownVendor(raw.trim().into()))
    }

    pub fn as_str(self) -> &'static str {
        profile(self).launch_name
    }
}

/// Values supplied by an effectful adapter, selected by the registry's closed
/// identity. Keeps adapter configuration local without duplicating vendor arms.
// trace:STORY-2326 | ai:codex
pub struct VendorValues<T> {
    pub claude: T,
    pub codex: T,
    pub agy: T,
}

impl<T> VendorValues<T> {
    pub fn get(self, vendor: LaunchVendor) -> T {
        match vendor {
            LaunchVendor::Claude => self.claude,
            LaunchVendor::Codex => self.codex,
            LaunchVendor::Agy => self.agy,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillSyntax {
    Slash,
    PortableMention,
    Render,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptTransport {
    Positional,
    Flag(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MailWake {
    PerTurnHooks,
    ExactThreadQueue,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resume {
    RecordedSession,
    RecordedThread,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Background {
    DaemonHandoff,
    HostDetached,
    Unsupported,
}

/// Candidate posture family, not an admission decision. AGY's outer-only
/// candidate still requires fresh all-tool conformance; it has no native credit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxSupport {
    LegacyAndShared,
    ExplicitNativeAndShared,
    OuterOnlyPendingConformance,
}

/// Installation policy; shared settings remain owned by the full scaffolder.
// trace:STORY-2539 | ai:codex
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookBackend {
    Settings,
    Standalone,
    NamedEntries,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookLayout {
    pub backend: HookBackend,
    pub template: &'static str,
    pub events: &'static [&'static str],
    pub wiring: &'static str,
    pub handlers: &'static str,
}

#[derive(Debug)]
pub struct VendorProfile {
    pub vendor: LaunchVendor,
    /// Legacy session/config spelling; never rewritten to the launch spelling.
    pub session_name: &'static str,
    pub launch_name: &'static str,
    pub aliases: &'static [&'static str],
    pub executable: &'static str,
    pub interactive_prompt: PromptTransport,
    pub hooks: HookLayout,
    pub sandbox: SandboxSupport,
    pub mail_wake: MailWake,
    pub resume: Resume,
    pub background: Background,
    pub skill_syntax: SkillSyntax,
}

/// One exhaustive table; effectful adapters refer to these same descriptors.
pub static PROFILES: [VendorProfile; 3] = [
    VendorProfile {
        vendor: LaunchVendor::Claude,
        session_name: "claude",
        launch_name: "claude",
        aliases: &["claude"],
        executable: "claude",
        interactive_prompt: PromptTransport::Positional,
        hooks: HookLayout {
            backend: HookBackend::Settings,
            template: "settings.json",
            events: &[
                "PreToolUse",
                "PostToolUse",
                "PreCompact", // STORY-2662: Claude only; Codex trusted-hook test pending.
                "SessionStart",
                "UserPromptSubmit",
                "Notification",
                "SubagentStart",
                "SubagentStop",
                "Stop",
            ],
            wiring: ".claude/settings.json",
            handlers: ".claude/hooks",
        },
        sandbox: SandboxSupport::LegacyAndShared,
        mail_wake: MailWake::PerTurnHooks,
        resume: Resume::RecordedSession,
        background: Background::DaemonHandoff,
        skill_syntax: SkillSyntax::Slash,
    },
    VendorProfile {
        vendor: LaunchVendor::Codex,
        session_name: "codex",
        launch_name: "codex",
        aliases: &["codex"],
        executable: "codex",
        interactive_prompt: PromptTransport::Positional,
        hooks: HookLayout {
            backend: HookBackend::Standalone,
            template: "codex-hooks.json",
            events: &[
                "PreToolUse",
                "PostToolUse",
                "SessionStart",
                "UserPromptSubmit",
                "SubagentStart",
                "SubagentStop",
                "Stop",
            ],
            wiring: ".codex/hooks.json",
            handlers: ".codex/hooks",
        },
        sandbox: SandboxSupport::ExplicitNativeAndShared,
        mail_wake: MailWake::ExactThreadQueue,
        resume: Resume::RecordedThread,
        background: Background::HostDetached,
        skill_syntax: SkillSyntax::PortableMention,
    },
    VendorProfile {
        vendor: LaunchVendor::Agy,
        session_name: "agy",
        launch_name: "antigravity",
        aliases: &["agy", "antigravity"],
        executable: "agy",
        interactive_prompt: PromptTransport::Flag("--prompt-interactive"),
        hooks: HookLayout {
            backend: HookBackend::NamedEntries,
            template: "antigravity-hooks.json",
            events: &["PreInvocation", "SessionStart"],
            wiring: ".agents/hooks.json",
            handlers: ".agents/hooks",
        },
        sandbox: SandboxSupport::OuterOnlyPendingConformance,
        mail_wake: MailWake::Unsupported,
        resume: Resume::Unsupported,
        background: Background::Unsupported,
        skill_syntax: SkillSyntax::Render,
    },
];

pub const fn profile(vendor: LaunchVendor) -> &'static VendorProfile {
    match vendor {
        LaunchVendor::Claude => &PROFILES[0],
        LaunchVendor::Codex => &PROFILES[1],
        LaunchVendor::Agy => &PROFILES[2],
    }
}

impl PromptTransport {
    /// Expand a prompt as argv data, without shell interpolation or tokenization.
    /// Positional callers put this tail after their options and separator.
    pub fn expand(self, prompt: String) -> Vec<String> {
        match self {
            Self::Positional => vec![prompt],
            Self::Flag(flag) => vec![flag.into(), prompt],
        }
    }
}

impl VendorProfile {
    pub fn interactive_prompt_args(&self, prompt: &str) -> Vec<String> {
        self.interactive_prompt.expand(prompt.into())
    }
}

mod headless;
pub mod headless_posture;

pub mod hooks;
