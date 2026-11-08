// trace:TASK-3424 | ai:codex
use super::*;

// trace:BUG-1530 | ai:claude
#[test]
fn unrecognised_seat_sees_every_owned_channel() {
    for role in ["integrator", "product", "human", "some-new-seat"] {
        let seat = classify_seat(Some(role));
        for owner in [
            AwaitingSeat::Reviewer,
            AwaitingSeat::Advisor,
            AwaitingSeat::Implementer,
        ] {
            assert!(
                owned_channel_visible(seat, owner),
                "role {role} must see {owner:?}"
            );
        }
    }
}

// trace:BUG-1530 | ai:claude
#[test]
fn role_matching_is_case_insensitive() {
    assert_eq!(
        classify_seat(Some("Reviewer")),
        Some(AwaitingSeat::Reviewer)
    );
    assert_eq!(classify_seat(Some("ADVISOR")), Some(AwaitingSeat::Advisor));
    assert!(owned_channel_visible(
        classify_seat(Some("Reviewer")),
        AwaitingSeat::Reviewer
    ));
}
