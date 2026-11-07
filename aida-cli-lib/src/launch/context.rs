//! [`LaunchRequest`] (untrusted intent) resolved once into a
//! [`LaunchContext`] (what will run). Resolution reads the parent's
//! containment and checks the caller-supplied argv; it has no effects, so a
//! refused launch leaves no lease, log, worktree or grant behind.
// trace:STORY-2326 | ai:claude

use std::path::{Path, PathBuf};

use aida_core::launch_policy::{LaunchKind, LaunchVendor, PostureFlag};

use crate::session::ContainmentPosture;

/// What a launch site asks for. In slice A the site still composes the
/// vendor argv (posture included) itself; `tail` is the caller-supplied part
/// (a prompt, `default_flags`) that may not carry posture.
#[derive(Debug, Clone)]
pub(crate) struct LaunchRequest {
    pub(crate) vendor: LaunchVendor,
    pub(crate) kind: LaunchKind,
    /// The program, already resolved (and OS-wrapped when contained).
    pub(crate) program: std::ffi::OsString,
    /// The vendor profile argv the site composed, posture flags included.
    pub(crate) args: Vec<String>,
    /// Caller-supplied argv appended after `args`; refused if it sets posture.
    pub(crate) tail: Vec<String>,
    pub(crate) posture: ContainmentPosture,
    /// The child's working directory, and the AIDA boundary's root.
    pub(crate) cwd: PathBuf,
    /// Whether the site sets the child's working directory itself
    /// (`Command::current_dir`). Sites that inherit this process's cwd say no.
    pub(crate) set_cwd: bool,
    /// The host main checkout the launcher pinned at its entry point, before
    /// any chdir into a seat clone. The boundary verifies the mailbox root
    /// against it; never derived from the process cwd below that point.
    // trace:TASK-2032 | ai:claude
    pub(crate) host_main: Option<PathBuf>,
}

/// Why a launch was refused, before any effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LaunchRefusal {
    /// The caller-supplied argv sets the sandbox or permission posture.
    PostureFlag(PostureFlag),
    /// This process runs contained and the child would not.
    WeakerThanParent {
        vendor: LaunchVendor,
        posture: ContainmentPosture,
    },
    /// `AIDA_CONTAINED` holds a value that is neither on nor off.
    MalformedParentContainment(String),
    /// An `aida` control child or a vendor query cannot be built here.
    UnsupportedKind(LaunchKind),
    /// This process runs contained and carries the `AIDA_AGENT_CMD` program
    /// override. A contained launch strips it from its child, so it can only
    /// have been set inside the boundary, where it would swap the vendor.
    AgentCmdUnderContainment,
    /// A legacy outer-only argv attempts bypass without the shared wrapper.
    // trace:TASK-3048 | ai:codex
    OuterOnlyPermission(String),
}

impl std::fmt::Display for LaunchRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PostureFlag(flag) => write!(f, "{flag}"),
            Self::OuterOnlyPermission(detail) => write!(f, "{detail}"),
            Self::WeakerThanParent { vendor, posture } => write!(
                f,
                "[contained] this session is contained, so it cannot launch a {} session that \
                 would run {}; refusing to launch it",
                vendor.as_str(),
                posture.label()
            ),
            Self::MalformedParentContainment(detail) => write!(f, "{detail}"),
            Self::UnsupportedKind(kind) => write!(
                f,
                "a {} launch is not an agent session; it has its own constructor",
                kind.as_str()
            ),
            Self::AgentCmdUnderContainment => write!(
                f,
                "[contained] this session is contained, so `AIDA_AGENT_CMD` cannot replace the \
                 agent program; unset it and launch again"
            ),
        }
    }
}

impl std::error::Error for LaunchRefusal {}

/// A resolved launch. Fields are private: only [`LaunchContext::resolve`]
/// builds one in production, so every launch passed the same checks.
#[derive(Debug, Clone)]
pub(crate) struct LaunchContext {
    vendor: LaunchVendor,
    kind: LaunchKind,
    program: std::ffi::OsString,
    argv: Vec<String>,
    posture: ContainmentPosture,
    cwd: PathBuf,
    set_cwd: bool,
    host_main: Option<PathBuf>,
    parent_contained: bool,
    fixture_override: bool,
}

impl LaunchContext {
    /// Resolve `req` against this process's own containment.
    pub(crate) fn resolve(req: LaunchRequest) -> Result<Self, LaunchRefusal> {
        // A malformed value refuses here; the answer is this process's own
        // inherited containment, never the export it made for its children.
        if let Err(err) = crate::session::aida_contained_env() {
            return Err(LaunchRefusal::MalformedParentContainment(format!(
                "{err:#}"
            )));
        }
        let agent_cmd = std::env::var_os(aida_core::launch_policy::AGENT_CMD_ENV)
            .is_some_and(|cmd| !cmd.to_string_lossy().trim().is_empty());
        Self::resolve_with(req, crate::session::process_contained(), agent_cmd)
    }

    /// [`Self::resolve`] with the parent's containment given and no program
    /// override. Tests only: production always resolves against this process.
    #[cfg(test)]
    pub(crate) fn resolve_under(
        req: LaunchRequest,
        parent_contained: bool,
    ) -> Result<Self, LaunchRefusal> {
        Self::resolve_with(req, parent_contained, false)
    }

    /// The pure resolution: `parent_contained` is this process's own
    /// containment and `agent_cmd` whether it carries an `AIDA_AGENT_CMD`
    /// override.
    pub(super) fn resolve_with(
        req: LaunchRequest,
        parent_contained: bool,
        agent_cmd: bool,
    ) -> Result<Self, LaunchRefusal> {
        if matches!(req.kind, LaunchKind::Query | LaunchKind::Control) {
            return Err(LaunchRefusal::UnsupportedKind(req.kind));
        }
        aida_core::launch_policy::refuse_posture_flags(&req.tail)
            .map_err(LaunchRefusal::PostureFlag)?;
        // Containment is monotonic: a contained parent never yields an
        // uncontained child. A Native (Codex, TASK-2631 opt-in) child keeps
        // today's rule until the Codex slice moves it onto the AIDA boundary.
        if parent_contained && req.posture == ContainmentPosture::None {
            return Err(LaunchRefusal::WeakerThanParent {
                vendor: req.vendor,
                posture: req.posture,
            });
        }
        // The override is a test seam for uncontained launches. A contained
        // child never inherits it (`spawn::aida_boundary` removes it), so a contained parent
        // that carries one set it inside the boundary.
        if parent_contained && agent_cmd {
            return Err(LaunchRefusal::AgentCmdUnderContainment);
        }
        // A posture marker alone is not an applied receipt. The wrapper adds
        // receipt-only options itself, after resolving this site-owned argv.
        // trace:TASK-3048 | ai:codex
        let mut argv = req.args;
        argv.extend(req.tail);
        let descriptor_key = match req.vendor {
            LaunchVendor::Agy => "agy",
            vendor => vendor.as_str(),
        };
        crate::containment::refuse_unreceipted_bypass(descriptor_key, &argv)
            .map_err(|error| LaunchRefusal::OuterOnlyPermission(error.to_string()))?;
        Ok(Self {
            vendor: req.vendor,
            kind: req.kind,
            program: req.program,
            argv,
            posture: req.posture,
            cwd: req.cwd,
            set_cwd: req.set_cwd,
            host_main: req.host_main,
            parent_contained,
            fixture_override: agent_cmd
                && !parent_contained
                && req.posture == ContainmentPosture::None,
        })
    }

    pub(crate) fn fixture_override(&self) -> bool {
        self.fixture_override
    }

    pub(crate) fn vendor(&self) -> LaunchVendor {
        self.vendor
    }

    pub(crate) fn kind(&self) -> LaunchKind {
        self.kind
    }

    pub(crate) fn program(&self) -> &std::ffi::OsStr {
        &self.program
    }

    pub(crate) fn argv(&self) -> &[String] {
        &self.argv
    }

    pub(crate) fn posture(&self) -> ContainmentPosture {
        self.posture
    }

    pub(crate) fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub(crate) fn sets_cwd(&self) -> bool {
        self.set_cwd
    }

    /// The launcher's pinned host main (see [`LaunchRequest::host_main`]).
    // trace:TASK-2032 | ai:claude
    pub(crate) fn host_main(&self) -> Option<&Path> {
        self.host_main.as_deref()
    }

    pub(crate) fn parent_contained(&self) -> bool {
        self.parent_contained
    }
}

/// The [`LaunchVendor`] of a headless vendor (the two enums agree by name).
pub(crate) fn launch_vendor(vendor: crate::session::HeadlessVendor) -> LaunchVendor {
    match vendor {
        crate::session::HeadlessVendor::Claude => LaunchVendor::Claude,
        crate::session::HeadlessVendor::Codex => LaunchVendor::Codex,
        crate::session::HeadlessVendor::Agy => LaunchVendor::Agy,
    }
}
