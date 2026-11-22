// trace:BUG-2629 | ai:codex
#[test]
fn bug_2629_queue_discovery_source_ratchet() {
    let cache = include_str!("rework_bindings.rs");
    assert_eq!(cache.matches("checked::discover_for_queue(").count(), 1);
    assert_eq!(cache.matches(".listed(spec)").count(), 1);
    let constructor = cache
        .split_once("fn discovery(")
        .unwrap()
        .1
        .split_once("    fn listed(")
        .unwrap()
        .0;
    assert!(constructor.contains("checked::discover_for_queue("));
    let work = include_str!("work.rs");
    let consumers = work
        .split_once("fn pickup_rework_review(")
        .unwrap()
        .1
        .split_once("fn headless_launch_hint(")
        .unwrap()
        .0;
    let waiting = include_str!("../review_waiting.rs")
        .split_once("fn reconcile_waiting_with(")
        .unwrap()
        .1
        .split_once("mod tests")
        .unwrap()
        .0;
    let verdict = include_str!("../gate/review_verdict.rs")
        .split_once("fn current_verdict_from_events(")
        .unwrap()
        .1
        .split_once("pub(crate) type HistoricalVerdict")
        .unwrap()
        .0;
    for source in [
        cache,
        consumers,
        waiting,
        verdict,
        include_str!("rework.rs"),
    ] {
        for forbidden in [
            "checked::discover(",
            "DiscoveryScope::enter(",
            "bound_changes_lossy(",
            "review_verdict::current_verdict(",
            "rv::current_verdict(",
            "review_waiting::reconcile_waiting_verdicts(",
            "project::changes_for_spec(",
            "project::index_changes(",
            "review_publication::current(",
        ] {
            assert!(
                !source.contains(forbidden),
                "queue review reopens evidence with {forbidden}"
            );
        }
    }
}
