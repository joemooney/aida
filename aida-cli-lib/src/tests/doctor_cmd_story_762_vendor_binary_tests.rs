// trace:TASK-3780 | ai:codex
use super::*;

// STORY-762: the codex-only doctor category resolves through the
// normalizer with its aliases; which_binary finds a real binary and
// returns None for a nonsense name.
#[test]
fn vendor_binary_category_normalizes_with_aliases() {
    for raw in ["vendor-binary", "vendor-binaries", "vendor", "VENDORS"] {
        assert_eq!(normalize_doctor_category(raw).unwrap(), "vendor-binary");
    }
}

#[test]
fn which_binary_finds_sh_and_misses_nonsense() {
    // `sh` exists on every unix CI runner; a random token does not.
    #[cfg(unix)]
    assert!(which_binary("sh").is_some());
    assert!(which_binary("definitely-not-a-real-binary-xyzzy").is_none());
}
