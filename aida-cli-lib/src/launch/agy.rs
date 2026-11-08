//! AGY adapter data for the shared AIDA boundary, pending all-tool proof.
//! No separate enforcement engine and no native-containment credit.
// trace:TASK-3048 | ai:codex
use anyhow::{Context, Result};
use std::path::Path;

/// Admission is separate from the common wrapper's shipped runtime data.
pub(crate) struct Adapter;
pub(crate) const AGY: Adapter = Adapter;

impl Adapter {
    /// No config value, synthetic receipt, or observed hostname can unlock this
    /// adapter. Removing this gate requires fresh integrated all-tool evidence,
    /// including the auth inventory and mediated-write positive controls.
    pub(crate) fn require_conformance(&self) -> Result<()> {
        anyhow::bail!(
            "agy shared AIDA boundary is pending fresh all-tool conformance for its outer-only posture; \
             raw Git/store writes are denied and require mediated host operations; \
             auth/refresh host inventory is incomplete; refusing to launch"
        )
    }

    /// Candidate inputs only; never a spawn path. Runtime/settings must live in
    /// launch-owned private state, with credentials mounted read-only by handle.
    /// This table does not inherit ~/.gemini, plugins, MCP, or daemon sockets.
    #[allow(dead_code)] // Used once the integrated adapter earns admission.
    pub(crate) fn candidate_argv(
        &self,
        private_runtime: &Path,
        prompt: &str,
        receipt: Option<&super::contained::ContainmentReceipt>,
    ) -> Result<Vec<String>> {
        // The signed outer-only posture permits this argv only with applied
        // evidence; it must never escape through a native/uncontained builder.
        super::contained::outer_only_argv("agy", private_runtime, prompt, receipt)
    }
}

fn shell_quote_diagnostic(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&b))
    {
        value.into()
    } else {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }
}

/// A deliberately named diagnostic entry, separate from queue/agent admission.
/// It runs only the closed fixture prompt and reuses every shared host barrier.
/// The candidate inventory may be measured here; production still refuses it.
// trace:TASK-3048 | ai:codex
pub(crate) fn diagnostic_launch(
    fixture: &Path,
    credential: &Path,
    parent: &aida_core::exec_context::Environment,
) -> Result<()> {
    use aida_core::launch_policy::LaunchKind;
    let fixture = std::fs::canonicalize(fixture)?;
    let root = std::fs::canonicalize(fixture.join("workspace"))?;
    let repo = std::fs::canonicalize(fixture.join("repository"))?;
    anyhow::ensure!(
        root.parent() == Some(fixture.as_path()) && repo.parent() == Some(fixture.as_path()),
        "AGY_DIAGNOSTIC_FIXTURE_INVALID: workspace and repository must be fixture siblings"
    );
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.join("aida-state.json"))?)?;
    anyhow::ensure!(
        state["report"]["boundary"] == "aida",
        "AGY_DIAGNOSTIC_FIXTURE_INVALID: expected agy AIDA-boundary fixture"
    );
    let vendor = state["report"]["vendor"]
        .as_str()
        .context("AGY_DIAGNOSTIC_FIXTURE_INVALID: missing vendor")?;
    // The shipped runtime descriptor is the closed selector. No arbitrary
    // program or settings template comes from fixture data.
    let policy = crate::containment::runtime_policy(vendor)?;
    anyhow::ensure!(
        credential.is_absolute()
            && credential.file_name() == Some(std::ffi::OsStr::new(&policy.credential))
            && credential.parent().and_then(Path::file_name)
                == Some(std::ffi::OsStr::new("antigravity-cli"))
            && credential
                .parent()
                .and_then(Path::parent)
                .and_then(Path::file_name)
                == Some(std::ffi::OsStr::new(".gemini")),
        "AGY_DIAGNOSTIC_CREDENTIAL_INVALID: expected exact conventional credential leaf"
    );
    let runner = fixture.join("payload/runner.py");
    anyhow::ensure!(
        runner.is_file(),
        "AGY_DIAGNOSTIC_FIXTURE_INVALID: runner missing"
    );
    let invocation = shell_quote_diagnostic(&runner.to_string_lossy());
    let manifest = shell_quote_diagnostic(&fixture.join("payload/manifest.json").to_string_lossy());
    let prompt = format!("Run exactly once with the terminal tool: /usr/bin/python3 {invocation} --await-payload {manifest}. \
        Use the current workspace as workdir. Then read the manifest and use your native file write/edit tool \
        (not terminal) to attempt each file_targets path with the nonce as content. Do not retry, change the payload, \
        use MCP/browser/URL/search/subagents, commit, publish or run queue done. Stop.");
    let grant =
        super::issue_launch_child(&repo, "implementer", &crate::current_user_id(None), true)?;
    struct Grant(String);
    impl Drop for Grant {
        fn drop(&mut self) {
            crate::session::revoke_headless_child_grant(&self.0);
        }
    }
    let grant = Grant(grant.id);
    let mut command = std::process::Command::new(vendor);
    super::contained::apply_transport_inputs(&mut command, parent);
    command
        .args(["-p", &prompt])
        .current_dir(&root)
        .env("AIDA_HEADLESS", "1")
        .env("AIDA_CONTAINED", "1")
        .env("AIDA_CONTAINMENT_POSTURE", "aida")
        .env("AIDA_PROJECT_ROOT", &root)
        .env("PWD", &root);
    super::assign_seat(
        &mut command,
        LaunchKind::Fresh,
        super::SeatDecision::Child {
            role: "implementer",
            grant: &grant.0,
        },
    )?;
    let mut run = super::contained::PreparedLaunch::prepare_runtime_diagnostic(
        &mut command,
        &root,
        credential,
    )?
    .spawn(std::process::Stdio::inherit())?;
    crate::gate::seat_authority::bind_contained_launch_worker(&grant.0, run.receipt())?;
    run.release()?;
    let status = run.child.wait()?;
    let status = run.vendor_exit(status)?;
    anyhow::ensure!(status.success(), "AGY_DIAGNOSTIC_VENDOR_FAILED: {status}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Check every central launch surface, including a forged Aida posture:
    // a context marker never substitutes for the receipt/exec barrier.
    // trace:TASK-3048 | ai:codex
    #[test]
    fn task_3048_legacy_bypass_refuses_before_command_construction() {
        use crate::launch::context::{LaunchContext, LaunchRefusal, LaunchRequest};
        use crate::session::ContainmentPosture;
        use aida_core::launch_policy::{LaunchKind, LaunchVendor};
        for kind in [
            LaunchKind::Fresh,
            LaunchKind::Exec,
            LaunchKind::Background,
            LaunchKind::Resume,
        ] {
            for posture in [
                ContainmentPosture::None,
                ContainmentPosture::Native,
                ContainmentPosture::Aida,
            ] {
                for flag in [
                    "--dangerously-skip-permissions",
                    "--dangerously-skip-permissions=true",
                    "-dangerously-skip-permissions",
                    "-dangerously-skip-permissions=true",
                ] {
                    // Defaults arrive in args; --extra-flag arrives in tail.
                    for in_tail in [false, true] {
                        let mut req = LaunchRequest {
                            vendor: LaunchVendor::Agy,
                            kind,
                            program: "agy".into(),
                            args: Vec::new(),
                            tail: Vec::new(),
                            posture,
                            cwd: std::env::temp_dir(),
                            set_cwd: false,
                            host_main: None,
                        };
                        if in_tail {
                            req.tail.push(flag.into());
                        } else {
                            req.args = vec![flag.into(), "-p".into(), "bounded task".into()];
                        }
                        let error = LaunchContext::resolve_under(req, false).unwrap_err();
                        // The generic tail gate may refuse before the outer-only
                        // gate, but no spelling or launch kind may reach a child.
                        if in_tail && flag.starts_with("--") {
                            assert!(matches!(error, LaunchRefusal::PostureFlag(_)));
                        } else {
                            assert!(matches!(error, LaunchRefusal::OuterOnlyPermission(_)));
                            assert!(error.to_string().contains(
                                "agy permission bypass requires an applied AIDA ContainmentReceipt"
                            ));
                        }
                    }
                }
            }
        }
    }

    // Native diagnostics remain available for every agent-session kind.
    // Query and Control retain their separate constructors.
    // trace:TASK-3048 | ai:codex
    #[test]
    fn task_3048_native_agy_without_bypass_resolves_for_each_launch_kind() {
        use crate::launch::context::{LaunchContext, LaunchRefusal, LaunchRequest};
        use crate::session::ContainmentPosture;
        use aida_core::launch_policy::{LaunchKind, LaunchVendor};
        for kind in [
            LaunchKind::Fresh,
            LaunchKind::Exec,
            LaunchKind::Background,
            LaunchKind::Resume,
            LaunchKind::Query,
            LaunchKind::Control,
        ] {
            let args = vec!["-p".into(), "bounded diagnostic".into()];
            let req = LaunchRequest {
                vendor: LaunchVendor::Agy,
                kind,
                program: "agy".into(),
                args: args.clone(),
                tail: Vec::new(),
                posture: ContainmentPosture::None,
                cwd: std::env::temp_dir(),
                set_cwd: false,
                host_main: None,
            };
            let result = LaunchContext::resolve_under(req, false);
            if matches!(kind, LaunchKind::Query | LaunchKind::Control) {
                assert_eq!(result.unwrap_err(), LaunchRefusal::UnsupportedKind(kind));
            } else {
                let context = result.unwrap();
                assert_eq!(context.vendor(), LaunchVendor::Agy);
                assert_eq!(context.kind(), kind);
                assert_eq!(context.argv(), args);
            }
        }
    }

    #[test]
    fn task_3048_prompt_text_cannot_select_permission_posture() {
        let policy = crate::containment::runtime_policy("agy").unwrap();
        for option in policy.prompt_options.iter().flat_map(|option| {
            [
                option.clone(),
                format!("-{}", option.trim_start_matches('-')),
            ]
        }) {
            for argv in [
                vec![option.clone(), "--dangerously-skip-permissions".into()],
                vec![format!("{option}=--dangerously-skip-permissions")],
            ] {
                crate::containment::refuse_unreceipted_bypass("agy", &argv).unwrap();
            }
        }
        // A prompt consumes one value; subsequent options still get checked.
        for prompt_option in ["-p", "-prompt", "-print", "-prompt-interactive"] {
            for flag in [
                "--dangerously-skip-permissions",
                "-dangerously-skip-permissions=true",
            ] {
                assert!(crate::containment::refuse_unreceipted_bypass(
                    "agy",
                    &[prompt_option.into(), "text".into(), flag.into()]
                )
                .is_err());
            }
        }
        // The signed ruling changes only vendors with native_layer=none.
        for vendor in ["claude", "codex"] {
            crate::containment::refuse_unreceipted_bypass(
                vendor,
                &["--dangerously-skip-permissions".into()],
            )
            .unwrap();
        }
    }

    #[test]
    fn task_3048_agent_defaults_refuse_legacy_bypass_without_contained_parent() {
        let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
        for (agent_type, flag) in ["agy", "antigravity"].into_iter().flat_map(|agent_type| {
            [
                "--dangerously-skip-permissions",
                "-dangerously-skip-permissions",
                "-dangerously-skip-permissions=true",
            ]
            .map(|flag| (agent_type, flag))
        }) {
            let config = crate::AgentLaunchConfig {
                delegate_seats: Vec::new(),
                agent_type,
                binary: "agy",
                default_args: vec![flag.into()],
                prompt_style: crate::AgentPromptStyle::Flag("--prompt-interactive"),
                contained: false,
            };
            let error = crate::refuse_uncontained_child_of_contained_parent(&config).unwrap_err();
            assert!(error.to_string().contains("legacy launch refused"));
        }
    }

    #[test]
    fn agy_adapter_keeps_unmeasured_policy_closed() {
        let policy = crate::containment::runtime_policy("agy").unwrap();
        assert!(!policy.auth_inventory_complete);
        assert_eq!(
            policy.auth_hosts,
            ["daily-cloudcode-pa.googleapis.com", "oauth2.googleapis.com"]
        );
        assert!(!policy.raw_git_writes && !policy.raw_store_writes);
        assert!(AGY
            .require_conformance()
            .unwrap_err()
            .to_string()
            .contains("fresh all-tool conformance"));
    }

    #[test]
    fn agy_adapter_outer_only_argv_refuses_without_applied_receipt() {
        let runtime = Path::new("/private/agy-runtime");
        let prompt = "--dangerously-skip-permissions is prompt text";
        assert!(AGY
            .candidate_argv(runtime, prompt, None)
            .unwrap_err()
            .to_string()
            .contains("applied AIDA ContainmentReceipt"));
        let policy = crate::containment::runtime_policy("agy").unwrap();
        assert!(!policy
            .argv_prefix
            .iter()
            .any(|s| s == "--dangerously-skip-permissions"));
        assert!(!policy
            .argv_prefix
            .iter()
            .any(|s| matches!(s.as_str(), "--mode" | "auto" | "--sandbox")));
        assert_eq!(policy.relative, ".gemini/antigravity-cli");
        assert_eq!(policy.credential, "antigravity-oauth-token");
        assert!(AGY.candidate_argv(Path::new("/"), prompt, None).is_err());
    }
}
