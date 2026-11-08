// trace:STORY-2326 | ai:codex
use super::*;

#[test]
fn story_2326_profiles_preserve_other_vendors_and_refuse_missing_codex_decision() {
    for vendor in [LaunchVendor::Claude, LaunchVendor::Agy] {
        let mut cmd = Command::new("fixture");
        cmd.args(["--", "prompt"])
            .env("AIDA_SESSION_ROLE", "fixture");
        let before = format!("{cmd:?}");
        VendorProfile::for_vendor(vendor)
            .finalize(&mut cmd)
            .unwrap();
        assert_eq!(format!("{cmd:?}"), before);
    }
    let mut cmd = Command::new("codex");
    cmd.env("AIDA_CONTAINMENT_POSTURE", "none");
    let error = VendorProfile::for_vendor(LaunchVendor::Codex)
        .finalize(&mut cmd)
        .unwrap_err();
    assert!(
        error.to_string().contains("no explicit grant decision"),
        "{error:#}"
    );
    assert!(cmd.get_args().next().is_none());
}

/// Removed env markers are absent from a cleared Command. Rebuilding argv
/// must preserve that state, including when a removed marker preceded clear.
// trace:STORY-2326 | ai:codex
#[test]
fn story_2326_codex_rebuild_preserves_env_clear() {
    let _env = crate::test_env::EnvVarsGuard::apply(&[("AIDA_CONTAINED", None)]);
    for cleared in [false, true] {
        let mut cmd = Command::new("codex");
        cmd.env_remove("STORY2326_REMOVED_MARKER");
        if cleared {
            cmd.env_clear();
        }
        cmd.args(["exec", "--", "prompt"])
            .env("AIDA_SESSION_GRANT", "")
            .env("AIDA_CONTAINMENT_POSTURE", "none");
        finalize_codex(&mut cmd).unwrap();
        cmd.env_remove("STORY2326_REMOVED_AFTER_REBUILD");
        assert_eq!(
            cmd.get_envs()
                .any(|(key, _)| key == "STORY2326_REMOVED_AFTER_REBUILD"),
            !cleared
        );
        assert!(cmd.get_args().any(|arg| arg == "prompt"));
    }
}

// trace:STORY-2326 | ai:codex
#[test]
fn story_2326_resource_env_uses_vendor_projection() {
    use super::super::resources::{CargoResources, SandboxResources, Sccache};
    use aida_core::{exec_context::Environment, launch_policy::LaunchKind};
    let resources = SandboxResources {
        write_roots: vec![],
        store_paths: vec![],
        deny: vec![],
        cargo: CargoResources {
            target_dir: "/seat/target".into(),
            slot_dir: None,
            sccache: Sccache::Off,
        },
    };
    let parent = Environment::from_pairs([
        ("RUSTC_WRAPPER".into(), "sccache".into()),
        ("CARGO_TARGET_DIR".into(), "/host/target".into()),
        ("UNLISTED_SECRET".into(), "secret".into()),
    ]);
    let env = VendorProfile::for_vendor(LaunchVendor::Claude)
        .child_env(LaunchKind::Fresh, &parent, Some(&resources))
        .unwrap();
    assert_eq!(env.get("RUSTC_WRAPPER"), Some(std::ffi::OsStr::new("")));
    assert_eq!(
        env.get("CARGO_TARGET_DIR"),
        Some(std::ffi::OsStr::new("/seat/target"))
    );
    assert!(env.get("UNLISTED_SECRET").is_none());
    // trace:STORY-2641 | ai:codex
    let mut cached = resources;
    cached.cargo.sccache = Sccache::Lane {
        wrapper: "/host/bin/sccache".into(),
        cache: "/lane/cache".into(),
        socket: "/lane/cache/server.sock".into(),
    };
    let env = VendorProfile::for_vendor(LaunchVendor::Claude)
        .child_env(LaunchKind::Fresh, &parent, Some(&cached))
        .unwrap();
    assert_eq!(
        env.get("SCCACHE_DIR"),
        Some(std::ffi::OsStr::new("/lane/cache"))
    );
    assert_eq!(env.get("CARGO_INCREMENTAL"), None);
}
