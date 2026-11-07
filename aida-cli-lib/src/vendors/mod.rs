//! Vendor-facing rendering over the core registry. No process construction,
//! admission decisions or grant issuance belongs in this module.
// trace:STORY-2539 | ai:codex
use aida_core::vendors::{Background, SkillSyntax, VendorProfile};

fn with_args(head: String, args: &str) -> String {
    match args.trim() {
        "" => head,
        args => format!("{head} {args}"),
    }
}

pub(crate) fn skill_ref(
    profile: &VendorProfile,
    skill: &str,
    args: &str,
    portable: bool,
) -> String {
    let skill = skill.trim_start_matches('/');
    match profile.skill_syntax {
        SkillSyntax::Slash => with_args(format!("/{skill}"), args),
        SkillSyntax::PortableMention if portable => with_args(format!("${skill}"), args),
        _ => {
            let render = format!("aida skill render {skill}");
            match args.trim() {
                "" => render,
                args => format!("{render}  # then follow it for {args}"),
            }
        }
    }
}

pub(crate) fn agent_new_cmd(profile: &VendorProfile, flags: &str, background: bool) -> String {
    let mut cmd = with_args(format!("aida agent new {}", profile.launch_name), flags);
    if background && profile.background == Background::DaemonHandoff {
        cmd.push_str(" --bg");
    }
    cmd
}

pub(crate) fn launch_with_skill(
    profile: &VendorProfile,
    skill: &str,
    args: &str,
    portable: bool,
) -> String {
    match profile.skill_syntax {
        SkillSyntax::Slash | SkillSyntax::PortableMention => format!(
            "{} '{}'",
            profile.executable,
            skill_ref(profile, skill, args, portable)
        ),
        SkillSyntax::Render => agent_new_cmd(profile, &format!("--spec {}", args.trim()), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::HeadlessVendor;
    use aida_core::vendors::{
        profile, LaunchVendor, MailWake, PromptTransport, Resume, SandboxSupport, PROFILES,
    };

    /// Snapshot the compatibility boundary, including prompt-as-data and the
    /// distinction between advertised capability and applied launch admission.
    // trace:STORY-2539 | ai:codex
    #[test]
    fn story_2539_registry_and_consumers_preserve_vendor_contracts() {
        let cases = [
            (
                HeadlessVendor::Claude,
                LaunchVendor::Claude,
                "claude",
                "claude",
                "claude",
                ".claude/settings.json",
                ".claude/hooks",
                MailWake::PerTurnHooks,
                Resume::RecordedSession,
                Background::DaemonHandoff,
                SkillSyntax::Slash,
                SandboxSupport::LegacyAndShared,
            ),
            (
                HeadlessVendor::Codex,
                LaunchVendor::Codex,
                "codex",
                "codex",
                "codex",
                ".codex/hooks.json",
                ".codex/hooks",
                MailWake::ExactThreadQueue,
                Resume::RecordedThread,
                Background::HostDetached,
                SkillSyntax::PortableMention,
                SandboxSupport::ExplicitNativeAndShared,
            ),
            (
                HeadlessVendor::Agy,
                LaunchVendor::Agy,
                "agy",
                "antigravity",
                "agy",
                ".agents/hooks.json",
                ".agents/hooks",
                MailWake::Unsupported,
                Resume::Unsupported,
                Background::Unsupported,
                SkillSyntax::Render,
                SandboxSupport::OuterOnlyPendingConformance,
            ),
        ];
        assert_eq!(PROFILES.len(), cases.len());
        for (
            legacy,
            key,
            session,
            launch,
            program,
            wiring,
            handlers,
            wake,
            resume,
            bg,
            syntax,
            sandbox,
        ) in cases
        {
            let p = profile(key);
            assert!(std::ptr::eq(legacy.profile(), p));
            assert_eq!(legacy.program(), program);
            assert_eq!(legacy.as_str(), session);
            assert_eq!(key.as_str(), launch);
            assert_eq!(p.hooks.wiring, wiring);
            assert_eq!(p.hooks.handlers, handlers);
            assert_eq!(p.mail_wake, wake);
            assert_eq!(p.resume, resume);
            assert_eq!(p.background, bg);
            assert_eq!(p.skill_syntax, syntax);
            assert_eq!(p.sandbox, sandbox);
            assert_eq!(
                crate::session::vendor_can_resume(legacy),
                resume != Resume::Unsupported
            );
            for alias in p.aliases {
                let raw = format!(" {} ", alias.to_uppercase());
                assert_eq!(LaunchVendor::parse(&raw), Ok(key));
                assert_eq!(HeadlessVendor::parse(&raw), Some(legacy));
            }
            for prompt in [
                "--settings",
                "--sandbox=none",
                "a 'quote' $dollar\nnew line",
                "",
            ] {
                let argv = p.interactive_prompt_args(prompt);
                assert_eq!(argv.last().unwrap(), prompt);
                match p.interactive_prompt {
                    PromptTransport::Positional => assert_eq!(argv, [prompt]),
                    PromptTransport::Flag(flag) => assert_eq!(argv, [flag, prompt]),
                }
            }
        }
        for refused in ["", "bash", "gemini", "opencode", "claude-code"] {
            assert!(LaunchVendor::parse(refused).is_err());
            assert!(HeadlessVendor::parse(refused).is_none());
        }
        // No mutation of persisted spellings or historical mailbox identities.
        assert_eq!(
            HeadlessVendor::parse("antigravity").unwrap().as_str(),
            "agy"
        );
        assert_eq!(LaunchVendor::parse("agy").unwrap().as_str(), "antigravity");
        use crate::mail_wake::WakeAdapter;
        for name in ["antigravity", "gemini"] {
            assert_eq!(WakeAdapter::for_vendor(name), WakeAdapter::Unsupported);
        }
        assert_eq!(WakeAdapter::for_vendor("codex"), WakeAdapter::Codex);
        // Mailbox normalization is deliberately broader/different than launch
        // parsing: raw `agy` retains its historical per-turn fallback.
        for name in ["claude", "agy", "historical-reader"] {
            assert_eq!(WakeAdapter::for_vendor(name), WakeAdapter::NotApplicable);
        }
        assert_eq!(
            crate::vendor_hints::skill_ref(HeadlessVendor::Claude, "aida-pr", "X"),
            "/aida-pr X"
        );
        assert_eq!(
            crate::vendor_hints::skill_ref(HeadlessVendor::Codex, "aida-pr", "X"),
            "$aida-pr X"
        );
        assert_eq!(
            crate::vendor_hints::skill_ref(HeadlessVendor::Codex, "aida-burndown", "X"),
            "aida skill render aida-burndown  # then follow it for X"
        );
        assert_eq!(
            agent_new_cmd(profile(LaunchVendor::Claude), "--spec X", true),
            "aida agent new claude --spec X --bg"
        );
        assert_eq!(
            agent_new_cmd(profile(LaunchVendor::Codex), "--spec X", true),
            "aida agent new codex --spec X"
        );
        assert_eq!(
            launch_with_skill(profile(LaunchVendor::Agy), "aida-pr", "X", true),
            "aida agent new antigravity --spec X"
        );
        assert!(crate::launch::agy::AGY.require_conformance().is_err());
        use aida_core::scaffolding::codex_hooks;
        assert_eq!(
            codex_hooks::HOOKS_JSON_REL,
            profile(LaunchVendor::Codex).hooks.wiring
        );
        assert_eq!(
            codex_hooks::HOOKS_DIR_REL,
            profile(LaunchVendor::Codex).hooks.handlers
        );
        assert_eq!(
            codex_hooks::AGY_HOOKS_JSON_REL,
            profile(LaunchVendor::Agy).hooks.wiring
        );
        assert_eq!(
            codex_hooks::AGY_HOOKS_DIR_REL,
            profile(LaunchVendor::Agy).hooks.handlers
        );
    }
}
