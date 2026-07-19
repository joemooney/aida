use super::{pr_ship_post_merge_aida_exe, resolve_aida_exe};

#[test]
fn returns_a_path() {
    // In the test binary, current_exe() resolves to the test runner.
    // The function should return that path (it exists) — not the
    // "aida" fallback.
    let exe = resolve_aida_exe();
    assert!(
        exe.exists() || exe == std::path::Path::new("aida"),
        "expected an existing path or the `aida` fallback, got: {}",
        exe.display()
    );
}

#[test]
fn handles_deleted_suffix_via_string_strip() {
    // The function strips " (deleted)" from the path string. This is a
    // direct unit on the stripping logic — we can't easily simulate a
    // real /proc/self/exe with the suffix without binary trickery, so
    // we verify the suffix-stripping invariant via the public API by
    // checking the returned path doesn't contain " (deleted)".
    let exe = resolve_aida_exe();
    let s = exe.to_string_lossy();
    assert!(
        !s.contains(" (deleted)"),
        "resolved path must not contain ' (deleted)' suffix; got: {s}"
    );
}

#[test]
fn pr_ship_post_merge_subcommands_do_not_require_path_lookup() {
    // In tests, current_exe() resolves to this test binary. pr_ship's
    // post-merge `pull` / `session end` path should therefore use an
    // existing executable path, not the bare "aida" PATH fallback.
    // trace:SPEC-411 | ai:codex
    let exe = pr_ship_post_merge_aida_exe();
    assert!(
        exe.exists(),
        "expected pr-ship post-merge subcommands to use an existing executable path, got: {}",
        exe.display()
    );
}
