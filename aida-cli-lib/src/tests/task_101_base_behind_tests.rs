use super::*;

// trace:TASK-101 | ai:claude
#[test]
fn zero_behind_is_silent() {
    assert_eq!(base_behind_indicator(0, 1), None);
    assert_eq!(base_behind_indicator(0, 5), None);
}

// trace:TASK-101 | ai:claude
#[test]
fn below_threshold_is_silent() {
    // Statusline default (5): a 1-4 commit drift stays quiet.
    assert_eq!(base_behind_indicator(1, 5), None);
    assert_eq!(base_behind_indicator(4, 5), None);
}

// trace:TASK-101 | ai:claude
#[test]
fn at_or_above_threshold_surfaces_count() {
    assert_eq!(
        base_behind_indicator(5, 5).as_deref(),
        Some("base behind by 5")
    );
    assert_eq!(
        base_behind_indicator(12, 5).as_deref(),
        Some("base behind by 12")
    );
}

// trace:TASK-101 | ai:claude
#[test]
fn queue_list_floor_surfaces_any_nonzero() {
    // `aida queue list` uses threshold 1: any non-zero drift shows.
    assert_eq!(
        base_behind_indicator(1, 1).as_deref(),
        Some("base behind by 1")
    );
    assert_eq!(
        base_behind_indicator(3, 1).as_deref(),
        Some("base behind by 3")
    );
    assert_eq!(base_behind_indicator(0, 1), None);
}

// trace:TASK-101 | ai:claude
#[test]
fn embeds_the_exact_count() {
    for n in [5u32, 6, 20, 99] {
        let label = base_behind_indicator(n, 5).expect("surfaces at/above threshold");
        assert!(label.contains(&n.to_string()), "{label} should contain {n}");
    }
}
