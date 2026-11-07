//! Headless syntax and settings composition from already resolved posture data.
//! Callers retain all admission checks and write-root discovery.
// trace:STORY-2539 | ai:codex
use super::headless_posture::headless_claude_posture;
use super::{LaunchVendor, VendorProfile};
use anyhow::Result;

impl VendorProfile {
    /// `None` preserves legacy bypass syntax. `Some(flags)` means the caller
    /// resolved a contained/native posture, including its exact write boundary.
    /// Supplying flags does not grant admission or applied-containment credit.
    pub fn headless_args(
        &self,
        prompt: &str,
        session_id: &str,
        model: Option<&str>,
        effort: Option<&str>,
        posture_flags: Option<Vec<String>>,
    ) -> Result<Vec<String>> {
        match self.vendor {
            LaunchVendor::Claude => {
                let permission_mode = if posture_flags.is_some() {
                    "dontAsk"
                } else {
                    "bypassPermissions"
                };
                let args: Vec<String> = vec![
                    "-p".to_string(),
                    "--permission-mode".to_string(),
                    permission_mode.to_string(),
                ]
                .into_iter()
                // trace:BUG-B6B1278890774EF993D2EBC327862470-050 | ai:claude
                .chain(headless_claude_posture(posture_flags.unwrap_or_default())?)
                .chain([
                    "--output-format".to_string(),
                    "stream-json".to_string(),
                    "--verbose".to_string(),
                    "--session-id".to_string(),
                    session_id.to_string(),
                ])
                .chain(
                    model
                        .filter(|m| !m.trim().is_empty())
                        .map(|m| vec!["--model".to_string(), m.to_string()])
                        .unwrap_or_default(),
                )
                .chain(
                    effort
                        .filter(|e| !e.trim().is_empty())
                        .map(|e| vec!["--effort".to_string(), e.to_string()])
                        .unwrap_or_default(),
                )
                .chain([prompt.to_string()])
                .collect();
                Ok(args)
            }
            LaunchVendor::Codex => {
                let mut args = vec![
                    "exec".to_string(),
                    "--json".to_string(),
                    "--dangerously-bypass-approvals-and-sandbox".to_string(),
                ];
                if let Some(flags) = posture_flags {
                    args.retain(|arg| arg != "--dangerously-bypass-approvals-and-sandbox");
                    args.splice(0..0, flags);
                }
                if let Some(model) = model.filter(|m| !m.trim().is_empty()) {
                    args.extend(["--model".to_string(), model.to_string()]);
                }
                if let Some(effort) = effort.filter(|e| !e.trim().is_empty()) {
                    args.extend(["-c".to_string(), format!("model_reasoning_effort={effort}")]);
                }
                args.push(prompt.to_string());

                Ok(args)
            }
            LaunchVendor::Agy => {
                // trace:TASK-2838 | ai:codex
                let mut args = vec![
                    "--dangerously-skip-permissions".to_string(),
                    "--output-format".to_string(),
                    "stream-json".to_string(),
                ];
                if let Some(effort) = effort.filter(|e| !e.trim().is_empty()) {
                    args.extend(["--effort".to_string(), effort.to_string()]);
                }
                args.extend(["-p".to_string(), prompt.to_string()]);

                Ok(args)
            }
        }
    }
}
