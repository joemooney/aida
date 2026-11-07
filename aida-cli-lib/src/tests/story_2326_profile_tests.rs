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
