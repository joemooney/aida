//! Regression tests for an open review story whose reviewer queue entry was lost.
//! trace:BUG-1230 | ai:codex

use super::*;

#[test]
fn open_unqueued_story_is_requeued_without_creating_a_duplicate() {
    assert_eq!(
        review_story_queue_decision(Some("STORY-1241".into()), &[]),
        ReviewStoryQueueDecision::Requeue("STORY-1241".into())
    );
}

#[test]
fn open_queued_story_is_skipped_as_already_queued() {
    assert_eq!(
        review_story_queue_decision(
            Some("STORY-1241".into()),
            &["STORY-17".into(), "story-1241".into()]
        ),
        ReviewStoryQueueDecision::Skip("STORY-1241".into())
    );
}

#[test]
fn absent_open_story_is_created() {
    assert_eq!(
        review_story_queue_decision(None, &["STORY-1241".into()]),
        ReviewStoryQueueDecision::Create
    );
}
