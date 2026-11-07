//! Existing headless posture resolution; syntax lives on the core profile.
// trace:STORY-2539 | ai:codex
use crate::session::claude_contained_flags_for_roots;
use crate::session::{
    claude_headless_args_with_posture_model_and_effort, contained_resume_write_roots,
    containment_applied, containment_posture, containment_posture_in, HeadlessVendor,
};
use anyhow::Result;
use std::path::{Path, PathBuf};

pub fn headless_vendor_args(
    host_main: Option<&Path>,
    vendor: HeadlessVendor,
    prompt: &str,
    session_id: &str,
    contained: bool,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<Vec<String>> {
    let actually_contained = containment_applied(vendor, contained, None);
    Ok(match vendor {
        HeadlessVendor::Claude => claude_headless_args_with_posture_model_and_effort(
            host_main,
            prompt,
            session_id,
            actually_contained,
            model,
            effort,
        )?,
        // trace:TASK-2631 | ai:codex
        HeadlessVendor::Codex => {
            let flags = if contained {
                containment_posture(vendor, true)?;
                Some(crate::tool_contained_flags(
                    host_main,
                    vendor.profile().executable,
                )?)
            } else {
                None
            };
            vendor
                .profile()
                .headless_args(prompt, session_id, model, effort, flags)?
        }
        HeadlessVendor::Agy => agy_headless_args_with_effort(prompt, effort),
    })
}

pub(crate) fn headless_vendor_args_contained_in(
    host_main: Option<&Path>,
    vendor: HeadlessVendor,
    prompt: &str,
    session_id: &str,
    contained_in: Option<&Path>,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<Vec<String>> {
    match (vendor, contained_in) {
        (HeadlessVendor::Claude, Some(dir)) => claude_headless_args_for_roots(
            host_main,
            prompt,
            session_id,
            Some(&contained_resume_write_roots(dir)),
            model,
            effort,
        ),
        // trace:TASK-2631 | ai:codex
        (HeadlessVendor::Codex, Some(dir)) => {
            containment_posture_in(dir, vendor.as_str(), true)?;
            let flags =
                crate::codex_native_sandbox::codex_native_sandbox_flags_in(host_main, dir, None)?;
            vendor
                .profile()
                .headless_args(prompt, session_id, model, effort, Some(flags))
        }
        _ => headless_vendor_args(host_main, vendor, prompt, session_id, false, model, effort),
    }
}

// trace:STORY-2539 | ai:codex
pub fn agy_headless_args_with_effort(prompt: &str, effort: Option<&str>) -> Vec<String> {
    aida_core::vendors::profile(aida_core::vendors::LaunchVendor::Agy)
        .headless_args(prompt, "", None, effort, None)
        .expect("headless syntax without supplied settings is valid")
}

// trace:STORY-2539 | ai:codex
pub fn codex_headless_args_with_model_and_effort(
    prompt: &str,
    model: Option<&str>,
    effort: Option<&str>,
) -> Vec<String> {
    aida_core::vendors::profile(aida_core::vendors::LaunchVendor::Codex)
        .headless_args(prompt, "", model, effort, None)
        .expect("headless syntax without supplied settings is valid")
}

// trace:STORY-2539 | ai:codex
pub(crate) fn claude_headless_args_for_roots(
    host_main: Option<&Path>,
    prompt: &str,
    session_id: &str,
    contained_roots: Option<&[PathBuf]>,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<Vec<String>> {
    let flags = contained_roots
        .map(|roots| claude_contained_flags_for_roots(host_main, roots))
        .transpose()?;
    aida_core::vendors::profile(aida_core::vendors::LaunchVendor::Claude)
        .headless_args(prompt, session_id, model, effort, flags)
}
