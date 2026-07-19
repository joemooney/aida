use super::should_reuse_branch;

#[test]
fn explicit_reuse_flag_always_reuses() {
    // --reuse-branch wins even for an auto-derived / absent branch.
    assert!(should_reuse_branch(true, false, false));
    assert!(should_reuse_branch(true, true, true));
}

#[test]
fn explicit_branch_that_exists_auto_reuses() {
    assert!(should_reuse_branch(false, true, true));
}

#[test]
fn explicit_branch_that_is_new_forks() {
    assert!(!should_reuse_branch(false, true, false));
}

#[test]
fn auto_derived_branch_never_reuses() {
    // branch_explicit == false → always fork, even on a (spurious)
    // preexists signal.
    assert!(!should_reuse_branch(false, false, true));
    assert!(!should_reuse_branch(false, false, false));
}
