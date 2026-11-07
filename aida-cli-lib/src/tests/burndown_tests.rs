// trace:TASK-3424 | ai:codex
    use super::*;

    fn cand(
        id: &str,
        req_type: &str,
        tags: &[&str],
        blocked: bool,
        decision: bool,
    ) -> BurndownCandidate {
        BurndownCandidate {
            id: id.to_string(),
            req_type: req_type.to_string(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            has_unsatisfied_blocker: blocked,
            has_pending_decision: decision,
        }
    }

    // TASK-803: the per-spec classifier — the filter/bucket ordering that was
    // previously inline (and untested) in `resolve_burndown_sets`.
    fn classify_case(
        archived: bool,
        deferred: bool,
        status: &str,
        want: &str,
        tags: &[&str],
        tag_filter: Option<&str>,
        batch_tag: Option<&str>,
    ) -> SpecDisposition {
        let tags: Vec<String> = tags.iter().map(|s| s.to_string()).collect();
        classify_spec(&SpecClassifyInput {
            archived,
            deferred,
            status_norm: status,
            want_status: want,
            tags: &tags,
            tag_filter,
            batch_tag,
            type_filter: None,
            disp: "TASK-1",
            req_type: "task",
            has_unsatisfied_blocker: false,
            has_pending_decision: false,
            execution_mode: None,
        })
    }

    /// STORY-618: the operator/advisor seat split must be a clean partition of
    /// the `needs_human` union — every needs-human bucket belongs to exactly
    /// one seat (or is `AwaitingDecision`, which the human view renders as its
    /// own first-class bucket), and the two seats never overlap.
    #[test]
    fn seat_split_partitions_needs_human() {
        use OpenBucket::*;
        let all = [
            HeldForReview,
            InFlight,
            AwaitingDecision,
            BuildSupervised,
            Deferred,
            Blocked,
            Umbrella,
            ReadyToClose,
            LongLived,
            Ungroomed,
            AwaitingMerge,
            InProgress,
            Actionable,
        ];
        for b in all {
            // Seats never overlap.
            assert!(
                !(b.advisor_seat() && b.operator_seat()),
                "{b:?} is in both seats"
            );
            // A seated bucket always needs a human.
            if b.advisor_seat() || b.operator_seat() {
                assert!(b.needs_human(), "{b:?} is seated but not needs_human");
            }
            // Every needs-human bucket is seated, except AwaitingDecision (its
            // own first-class human-view bucket).
            if b.needs_human() && b != AwaitingDecision {
                assert!(
                    b.advisor_seat() ^ b.operator_seat(),
                    "{b:?} needs a human but isn't in exactly one seat"
                );
            }
        }
        // Spot-check the seat assignments the user complaint hinges on.
        assert!(Ungroomed.advisor_seat(), "drafts groom on the advisor seat");
        assert!(ReadyToClose.advisor_seat());
        assert!(Umbrella.advisor_seat());
        assert!(HeldForReview.operator_seat());
        assert!(BuildSupervised.operator_seat());
    }

    // BUG-572: the `aida human` operator-view filter ORs in the `human_only`
    // marker (BUG-564) so a human-only spec surfaces even when its derived
    // bucket isn't a needs-human one. But the marker must NOT override the
    // to-groom→advisor routing: an ungroomed Draft (the `Ungroomed` bucket) with
    // `human_only=true` is grooming work, not a decision — it belongs on the
    // advisor, never on the operator. A GROOMED human_only spec (any non-
    // `Ungroomed` bucket — e.g. BUG-564's High/Approved scaffolding task) STILL
    // reaches the operator. This encodes the exact `human_only` clause from
    // `handle_list_human`'s `classified` filter. trace:BUG-572 | ai:claude
    #[test]
    fn human_only_does_not_pull_ungroomed_draft_onto_operator_seat() {
        // The operator-view `human_only` clause, lifted from
        // `handle_list_human`: human_only-marked specs surface here EXCEPT when
        // their bucket is the to-groom (`Ungroomed`) bucket.
        let human_clause =
            |human_only: bool, bucket: OpenBucket| human_only && bucket != OpenBucket::Ungroomed;

        // An ungroomed to-groom draft marked human_only stays OFF the operator
        // seat — grooming is advisor work.
        assert!(
            !human_clause(true, OpenBucket::Ungroomed),
            "an ungroomed (to-groom) human_only draft must not reach the operator"
        );
        // ...and it IS routed to the advisor: the advisor-worklist filter gates
        // on advisor_seat(), which Ungroomed satisfies — so it is not dropped
        // from both views.
        assert!(
            OpenBucket::Ungroomed.advisor_seat(),
            "the ungroomed draft must surface on the advisor seat instead"
        );

        // BUG-564 regression guard: a GROOMED human_only spec (non-Ungroomed
        // bucket) STILL reaches the operator via the human_only clause.
        assert!(
            human_clause(true, OpenBucket::HeldForReview),
            "a groomed human_only spec must still reach the operator (BUG-564)"
        );
        assert!(
            human_clause(true, OpenBucket::Actionable),
            "a groomed (non-Ungroomed) human_only spec must still reach the operator"
        );
    }

    // BUG-579: the phantom "N items need a human" bug — a `classified`
    // AwaitingDecision row with NO posed DecisionRequest is COUNTED toward the
    // total but, before the fix, was rendered by neither the ordered derived-
    // bucket loop nor the decisions-awaiting bucket (which only lists rows with a
    // real DecisionRequest). `aida human`'s catch-all renders exactly the rows
    // for which `classified_row_is_rendered` returns false, so this asserts that
    // such a row IS surfaced (count==render). The test FAILS against the old code
    // (AwaitingDecision was excluded from `order` AND from decisions-awaiting
    // when has_pending_decision==false → invisible). trace:BUG-579 | ai:claude
    #[test]
    fn awaiting_decision_without_posed_question_is_rendered_not_just_counted() {
        // The exact case from BUG-579: a Spike classified AwaitingDecision with
        // no formal DecisionRequest. It is NOT covered by the ordered buckets and
        // NOT covered by the decisions-awaiting bucket → must be caught by the
        // catch-all (predicate returns false → catch-all renders it).
        assert!(
            !classified_row_is_rendered(OpenBucket::AwaitingDecision, false),
            "AwaitingDecision with no DecisionRequest must NOT be considered \
             already-rendered — the catch-all must surface it (BUG-579)"
        );
        // It gets the actionable, decision-specific hint, not the generic one.
        assert!(
            needs_attention_hint(OpenBucket::AwaitingDecision).contains("decision"),
            "the needs-attention hint must point the operator at posing a decision"
        );

        // The complement: an AwaitingDecision row WITH a posed DecisionRequest is
        // already rendered by the first-class decisions-awaiting bucket — the
        // catch-all must NOT double-render it.
        assert!(
            classified_row_is_rendered(OpenBucket::AwaitingDecision, true),
            "AwaitingDecision WITH a DecisionRequest is shown by the decisions \
             bucket — the catch-all must not re-render it"
        );

        // The predicate must stay the exact mirror of the render order — if a
        // bucket is in `HUMAN_DERIVED_BUCKET_ORDER` it is rendered by the ordered
        // loop; otherwise it is rendered only via decisions-awaiting (and only
        // with a posed question) or, failing that, the catch-all. This guards the
        // predicate from drifting away from the loop and silently re-introducing
        // the count!=render gap. trace:BUG-579
        use OpenBucket::*;
        for b in HUMAN_DERIVED_BUCKET_ORDER {
            assert!(
                classified_row_is_rendered(b, false),
                "{b:?} is in the render order but the predicate calls it unrendered"
            );
        }
        // Buckets NOT in the order array (other than AwaitingDecision-with-a-
        // question) are reported unrendered → the catch-all is responsible for
        // them. Spot-check the variants that can reach `classified` outside the
        // ordered set.
        for b in [
            InFlight,
            Deferred,
            Blocked,
            LongLived,
            AwaitingMerge,
            InProgress,
            Actionable,
        ] {
            assert!(
                !classified_row_is_rendered(b, false),
                "{b:?} is outside the ordered loop — the catch-all must own it"
            );
        }
    }

    #[test]
    fn classify_skips_archived_deferred_and_status_mismatch() {
        assert_eq!(
            classify_case(true, false, "approved", "approved", &[], None, None),
            SpecDisposition::Skip
        );
        assert_eq!(
            classify_case(false, true, "approved", "approved", &[], None, None),
            SpecDisposition::Skip
        );
        assert_eq!(
            classify_case(false, false, "done", "approved", &[], None, None),
            SpecDisposition::Skip
        );
    }

    #[test]
    fn classify_supervised_only_when_status_matches() {
        // BUG-551: the supervised check runs AFTER the status filter, so a
        // supervised spec only buckets as Supervised when it matches want_status.
        assert_eq!(
            classify_case(
                false,
                false,
                "approved",
                "approved",
                &["supervised"],
                None,
                None
            ),
            SpecDisposition::Supervised
        );
        // Done / Completed supervised specs fail the status guard first → Skip
        // (they belong in the reviews bucket / are shipped, not "work this").
        assert_eq!(
            classify_case(
                false,
                false,
                "done",
                "approved",
                &["supervised"],
                None,
                None
            ),
            SpecDisposition::Skip
        );
        assert_eq!(
            classify_case(
                false,
                false,
                "completed",
                "approved",
                &["supervised"],
                None,
                None
            ),
            SpecDisposition::Skip
        );
        // Tag match is case-insensitive.
        assert_eq!(
            classify_case(
                false,
                false,
                "approved",
                "approved",
                &["Supervised"],
                None,
                None
            ),
            SpecDisposition::Supervised
        );
    }

    #[test]
    fn classify_honors_tag_and_batch_filters_before_supervised() {
        // --tag mismatch → Skip even if otherwise in scope.
        assert_eq!(
            classify_case(
                false,
                false,
                "approved",
                "approved",
                &["foo"],
                Some("bar"),
                None
            ),
            SpecDisposition::Skip
        );
        assert!(matches!(
            classify_case(
                false,
                false,
                "approved",
                "approved",
                &["bar"],
                Some("bar"),
                None
            ),
            SpecDisposition::Candidate(_)
        ));
        // --batch mismatch → Skip; match → Candidate.
        assert_eq!(
            classify_case(
                false,
                false,
                "approved",
                "approved",
                &["batch:other"],
                None,
                Some("batch:x")
            ),
            SpecDisposition::Skip
        );
        assert!(matches!(
            classify_case(
                false,
                false,
                "approved",
                "approved",
                &["batch:x"],
                None,
                Some("batch:x")
            ),
            SpecDisposition::Candidate(_)
        ));
        // A supervised spec that fails the --tag filter is Skipped, not
        // Supervised — the filter precedes the supervised bucket.
        assert_eq!(
            classify_case(
                false,
                false,
                "approved",
                "approved",
                &["supervised"],
                Some("bar"),
                None
            ),
            SpecDisposition::Skip
        );
    }

    /// BUG-784: an accepted ADR sits at `Approved` exactly like blessed work,
    /// so without a type-class filter the selector offered it as an
    /// implementable candidate. A repo full of ADRs / visions / terms /
    /// principles must yield ZERO candidates.
    // trace:BUG-784 | ai:claude
    #[test]
    fn knowledge_class_types_are_never_candidates_by_default() {
        // FAQ is reference, not work. trace:STORY-1642 | ai:claude
        for ty in ["decision", "vision", "term", "principle", "faq"] {
            let d = classify_spec(&SpecClassifyInput {
                archived: false,
                deferred: false,
                status_norm: "approved",
                want_status: "approved",
                tags: &[],
                tag_filter: None,
                batch_tag: None,
                type_filter: None,
                disp: "ADR-1",
                req_type: ty,
                has_unsatisfied_blocker: false,
                has_pending_decision: false,
                execution_mode: None,
            });
            assert_eq!(d, SpecDisposition::Skip, "{ty} must not be a candidate");
        }
    }

    /// BUG-1717: a spec whose groomed execution_mode needs a human in the loop
    /// must bucket as `Supervised`, never as a fan-out candidate. Before the
    /// fix only the `supervised` TAG could reach that bucket, so STORY-1484
    /// (mode guided, heft 17, cache-refresh architecture) was listed as "Ready
    /// to fan out" while the supervised count read 0 — a drain would have
    /// dispatched a keystone unattended.
    // trace:BUG-1717 | ai:claude
    #[test]
    fn human_in_the_loop_execution_modes_are_supervised_never_candidates() {
        for mode in [
            aida_core::ExecutionMode::Guided,
            aida_core::ExecutionMode::Operator,
            aida_core::ExecutionMode::Decide,
        ] {
            let d = classify_spec(&SpecClassifyInput {
                archived: false,
                deferred: false,
                status_norm: "approved",
                want_status: "approved",
                tags: &[],
                tag_filter: None,
                batch_tag: None,
                type_filter: None,
                disp: "STORY-1484",
                req_type: "story",
                has_unsatisfied_blocker: false,
                has_pending_decision: false,
                execution_mode: Some(mode),
            });
            assert_eq!(
                d,
                SpecDisposition::Supervised,
                "mode {mode} must be supervised, not fanned out"
            );
        }
    }

    /// BUG-1717: the autonomous rungs stay autonomous. Guarding the fix against
    /// over-correction — if `drain`/`drive` also became Supervised the drain
    /// would have nothing to do, which would "fix" the bug by breaking the
    /// feature.
    // trace:BUG-1717 | ai:claude
    #[test]
    fn autonomous_execution_modes_remain_candidates() {
        for mode in [
            aida_core::ExecutionMode::Drain,
            aida_core::ExecutionMode::Drive,
        ] {
            let d = classify_spec(&SpecClassifyInput {
                archived: false,
                deferred: false,
                status_norm: "approved",
                want_status: "approved",
                tags: &[],
                tag_filter: None,
                batch_tag: None,
                type_filter: None,
                disp: "TASK-204",
                req_type: "task",
                has_unsatisfied_blocker: false,
                has_pending_decision: false,
                execution_mode: Some(mode),
            });
            assert!(
                matches!(d, SpecDisposition::Candidate(_)),
                "mode {mode} must stay a fan-out candidate, got {d:?}"
            );
        }
    }

    /// BUG-1717: an UNGROOMED spec (no mode) keeps its previous disposition.
    /// It is not supervised — it is simply not yet groomed — and pinning this
    /// keeps the fix's blast radius to the human-in-the-loop modes. That
    /// `burndown plan` still advertises an ungroomed spec that `aida do` will
    /// refuse is a separate labelling defect, tracked on BUG-1717, not changed
    /// here.
    // trace:BUG-1717 | ai:claude
    #[test]
    fn ungroomed_spec_disposition_is_unchanged_by_the_mode_gate() {
        let d = classify_spec(&SpecClassifyInput {
            archived: false,
            deferred: false,
            status_norm: "approved",
            want_status: "approved",
            tags: &[],
            tag_filter: None,
            batch_tag: None,
            type_filter: None,
            disp: "TASK-1",
            req_type: "task",
            has_unsatisfied_blocker: false,
            has_pending_decision: false,
            execution_mode: None,
        });
        assert!(matches!(d, SpecDisposition::Candidate(_)), "got {d:?}");
    }

    /// BUG-784: a knowledge-class record tagged `supervised` must be dropped
    /// outright, not bucketed for keyboard pickup — the type filter sits ahead
    /// of the supervised bucket for exactly this reason.
    // trace:BUG-784 | ai:claude
    #[test]
    fn knowledge_class_type_does_not_reach_the_supervised_bucket() {
        let tags = vec!["supervised".to_string()];
        let d = classify_spec(&SpecClassifyInput {
            archived: false,
            deferred: false,
            status_norm: "approved",
            want_status: "approved",
            tags: &tags,
            tag_filter: None,
            batch_tag: None,
            type_filter: None,
            disp: "ADR-2",
            req_type: "decision",
            has_unsatisfied_blocker: false,
            has_pending_decision: false,
            execution_mode: None,
        });
        assert_eq!(d, SpecDisposition::Skip);
    }

    /// BUG-784: naming the type explicitly is the opt-back-in, and it narrows
    /// the view to that type (a work spec falls out of a `--type decision` run).
    // trace:BUG-784 | ai:claude
    #[test]
    fn explicit_type_filter_opts_a_knowledge_class_type_back_in() {
        let case = |req_type: &str, want_type: Option<&str>| {
            classify_spec(&SpecClassifyInput {
                archived: false,
                deferred: false,
                status_norm: "approved",
                want_status: "approved",
                tags: &[],
                tag_filter: None,
                batch_tag: None,
                type_filter: want_type,
                disp: "X-1",
                req_type,
                has_unsatisfied_blocker: false,
                has_pending_decision: false,
                execution_mode: None,
            })
        };
        assert!(matches!(
            case("decision", Some("decision")),
            SpecDisposition::Candidate(_)
        ));
        assert_eq!(case("task", Some("decision")), SpecDisposition::Skip);
        assert!(matches!(
            case("task", Some("task")),
            SpecDisposition::Candidate(_)
        ));
    }

    /// BUG-784: every non-knowledge type keeps flowing through, including the
    /// multi-word ones whose normalized token is a single word.
    // trace:BUG-784 | ai:claude
    #[test]
    fn work_types_are_unaffected_by_the_type_class_filter() {
        for ty in [
            "functional",
            "nonfunctional",
            "system",
            "user",
            "changerequest",
            "bug",
            "epic",
            "story",
            "task",
            "spike",
            "sprint",
            "folder",
            "meta",
            "constraint",
            "doc",
            // An unknown token is never silently dropped.
            "somefuturetype",
        ] {
            let d = classify_spec(&SpecClassifyInput {
                archived: false,
                deferred: false,
                status_norm: "approved",
                want_status: "approved",
                tags: &[],
                tag_filter: None,
                batch_tag: None,
                type_filter: None,
                disp: "X-1",
                req_type: ty,
                has_unsatisfied_blocker: false,
                has_pending_decision: false,
                execution_mode: None,
            });
            assert!(
                matches!(d, SpecDisposition::Candidate(_)),
                "{ty} should still be a candidate, got {d:?}"
            );
        }
    }

    #[test]
    fn classify_plain_matching_spec_is_candidate_carrying_probed_fields() {
        let tags = vec!["papercut".to_string()];
        let d = classify_spec(&SpecClassifyInput {
            archived: false,
            deferred: false,
            status_norm: "approved",
            want_status: "approved",
            tags: &tags,
            tag_filter: None,
            batch_tag: None,
            type_filter: None,
            disp: "TASK-9",
            req_type: "task",
            has_unsatisfied_blocker: true,
            has_pending_decision: true,
            execution_mode: None,
        });
        match d {
            SpecDisposition::Candidate(c) => {
                assert_eq!(c.id, "TASK-9");
                assert_eq!(c.req_type, "task");
                assert!(c.has_unsatisfied_blocker);
                assert!(c.has_pending_decision);
                assert_eq!(c.tags, vec!["papercut".to_string()]);
            }
            other => panic!("expected Candidate, got {other:?}"),
        }
    }

    #[test]
    fn bounded_unblocked_decision_free_spec_is_ready() {
        assert_eq!(
            classify(&cand("TASK-1", "task", &["papercut"], false, false)),
            Pickability::Ready
        );
    }

    #[test]
    fn epic_is_parked_for_decomposition() {
        match classify(&cand("EPIC-1", "epic", &[], false, false)) {
            Pickability::Parked(r) => assert!(r.contains("decompose")),
            other => panic!("expected Parked, got {other:?}"),
        }
    }

    #[test]
    fn pending_decision_parks() {
        assert!(matches!(
            classify(&cand("STORY-1", "story", &[], false, true)),
            Pickability::Parked(_)
        ));
    }

    #[test]
    fn unsatisfied_blocker_parks() {
        assert!(matches!(
            classify(&cand("TASK-2", "task", &[], true, false)),
            Pickability::Parked(_)
        ));
    }

    #[test]
    fn parking_tags_park_case_insensitively_with_deferred_prefix() {
        for tag in [
            "blocked",
            "needs-human-input",
            "Needs-Human",
            "needs-design-signoff",
            "operator-action",
            "needs-decision",
            "needs-supervised-build",
            "review:draft-only",
            "deferred:post-stability",
        ] {
            match classify(&cand("X-1", "task", &[tag], false, false)) {
                Pickability::Parked(r) => assert!(r.to_lowercase().contains(&tag.to_lowercase())),
                other => panic!("tag {tag} should park, got {other:?}"),
            }
        }
        // A benign tag does not park.
        assert_eq!(
            classify(&cand(
                "X-2",
                "task",
                &["batch:foo", "papercut"],
                false,
                false
            )),
            Pickability::Ready
        );
    }

    // STORY-568: spikes are parked from the implementer fan-out, but with a
    // precise lane reason — never the old flat "human-only" for agent-able
    // research.
    #[test]
    fn untagged_spike_is_research_lane_not_human_only() {
        match classify(&cand("SPIKE-1", "spike", &[], false, false)) {
            Pickability::Parked(r) => {
                assert!(r.contains("research-lane"), "got {r:?}");
                assert!(
                    r.contains("aida research"),
                    "names the dispatch path: {r:?}"
                );
                assert!(!r.to_lowercase().contains("human-only"), "got {r:?}");
            }
            other => panic!("a spike should park, got {other:?}"),
        }
    }

    #[test]
    fn spike_lane_split_by_tag() {
        assert_eq!(classify_spike_lane(&[]), SpikeLane::Research);
        assert_eq!(
            classify_spike_lane(&["needs-supervised-build".into()]),
            SpikeLane::Research
        );
        assert_eq!(
            classify_spike_lane(&["needs-decision".into()]),
            SpikeLane::NeedsDecision
        );
        assert_eq!(
            classify_spike_lane(&["Human-Only".into()]),
            SpikeLane::HumanOnly
        );
        // human-only is the most specific human gate and wins the tie.
        assert_eq!(
            classify_spike_lane(&["needs-decision".into(), "human-only".into()]),
            SpikeLane::HumanOnly
        );
    }

    #[test]
    fn needs_decision_spike_parks_as_decision_lane() {
        match classify(&cand("SPIKE-2", "spike", &["needs-decision"], false, false)) {
            Pickability::Parked(r) => assert!(r.contains("needs-decision"), "got {r:?}"),
            other => panic!("expected Parked, got {other:?}"),
        }
    }

    // trace:BUG-514 | ai:codex
    #[test]
    fn deferred_spike_reports_parking_tag_before_research_lane() {
        match classify(&cand(
            "SPIKE-4",
            "spike",
            &["deferred:stabilization-first"],
            false,
            false,
        )) {
            Pickability::Parked(r) => {
                assert!(r.contains("deferred:stabilization-first"), "got {r:?}");
                assert!(!r.contains("research-lane"), "got {r:?}");
                assert!(!r.contains("aida research"), "got {r:?}");
            }
            other => panic!("expected Parked, got {other:?}"),
        }
    }

    #[test]
    fn spike_with_pending_decision_reports_the_pending_question_first() {
        // Research already escalated -> the pending-decision reason (more
        // fundamental) wins over the lane reason.
        match classify(&cand("SPIKE-3", "spike", &[], false, true)) {
            Pickability::Parked(r) => assert!(r.contains("pending decision"), "got {r:?}"),
            other => panic!("expected Parked, got {other:?}"),
        }
    }

    #[test]
    fn partition_preserves_order_and_separates() {
        let cands = vec![
            cand("A", "task", &[], false, false),             // ready
            cand("B", "epic", &[], false, false),             // parked (epic)
            cand("C", "task", &["deferred:x"], false, false), // parked (tag)
            cand("D", "story", &[], false, false),            // ready
        ];
        let (ready, parked) = partition(&cands);
        assert_eq!(ready, vec!["A".to_string(), "D".to_string()]);
        assert_eq!(
            parked.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
            vec!["B".to_string(), "C".to_string()]
        );
    }

    // TASK-1172 / TASK-1175: the ready-set ordering sort. A pure, total sort
    // over the already-computed ready ids — priority band first (default), with
    // queue-insertion time (`added_at`) as the tiebreak, or insertion time alone
    // under `order = queue`.
    fn prios(pairs: &[(&str, &str)]) -> std::collections::HashMap<String, String> {
        pairs
            .iter()
            .map(|(id, p)| (id.to_string(), p.to_string()))
            .collect()
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    // `added_at` fixtures as "seconds after an arbitrary epoch" — the numbers
    // only have to be ordered relative to each other. trace:TASK-1175
    fn queued_at(
        pairs: &[(&str, i64)],
    ) -> std::collections::HashMap<String, chrono::DateTime<chrono::Utc>> {
        pairs
            .iter()
            .map(|(id, secs)| {
                (
                    id.to_string(),
                    chrono::DateTime::from_timestamp(1_750_000_000 + secs, 0).unwrap(),
                )
            })
            .collect()
    }

    fn no_added_at() -> std::collections::HashMap<String, chrono::DateTime<chrono::Utc>> {
        std::collections::HashMap::new()
    }

    #[test]
    fn ready_set_fans_high_priority_first_regardless_of_queue_position() {
        // A high-priority bug queued LAST still drains first.
        let ready = ids(&["TASK-1", "TASK-2", "BUG-9", "TASK-3"]);
        let p = prios(&[
            ("TASK-1", "Low"),
            ("TASK-2", "Medium"),
            ("BUG-9", "High"),
            ("TASK-3", "Medium"),
        ]);
        let q = queued_at(&[
            ("TASK-1", 10),
            ("TASK-2", 20),
            ("BUG-9", 30),
            ("TASK-3", 40),
        ]);
        assert_eq!(
            sort_ready(ready, &p, &q, ReadyOrder::Priority),
            ids(&["BUG-9", "TASK-2", "TASK-3", "TASK-1"])
        );
    }

    // TASK-1175 acceptance: `order = queue` drains in the order specs were
    // QUEUED — a lexically-later id queued earlier goes first.
    #[test]
    fn queue_order_drains_by_added_at_not_spec_id() {
        // "TASK-9" is lexically LATER than "TASK-1"/"TASK-2" but was queued
        // FIRST, so it must drain first.
        let ready = ids(&["TASK-1", "TASK-2", "TASK-9"]);
        let p = prios(&[("TASK-1", "High"), ("TASK-2", "High"), ("TASK-9", "Low")]);
        let q = queued_at(&[("TASK-9", 5), ("TASK-2", 15), ("TASK-1", 25)]);
        assert_eq!(
            sort_ready(ready, &p, &q, ReadyOrder::Queue),
            ids(&["TASK-9", "TASK-2", "TASK-1"]),
            "queue order is insertion order, and priority must not reorder it"
        );
    }

    #[test]
    fn queue_order_is_independent_of_the_incoming_vec_order() {
        // Same set, shuffled on the way in → same drained order out. The sort is
        // a fact about the queue, not about how the caller happened to collect.
        let p = prios(&[("B-2", "Medium"), ("A-1", "Medium"), ("C-3", "Medium")]);
        let q = queued_at(&[("C-3", 1), ("A-1", 2), ("B-2", 3)]);
        let expected = ids(&["C-3", "A-1", "B-2"]);
        assert_eq!(
            sort_ready(ids(&["A-1", "B-2", "C-3"]), &p, &q, ReadyOrder::Queue),
            expected
        );
        assert_eq!(
            sort_ready(ids(&["B-2", "C-3", "A-1"]), &p, &q, ReadyOrder::Queue),
            expected
        );
    }

    // TASK-1175 acceptance: within a priority band the tiebreak is `added_at`,
    // NOT lexical spec-id order.
    #[test]
    fn equal_priority_breaks_ties_by_queue_insertion_time_not_id() {
        // Incoming id order and lexical id order both say TASK-2 < TASK-5 <
        // TASK-7; insertion order says the opposite. Insertion order wins.
        let ready = ids(&["TASK-2", "TASK-5", "TASK-7"]);
        let p = prios(&[("TASK-7", "High"), ("TASK-2", "High"), ("TASK-5", "High")]);
        let q = queued_at(&[("TASK-7", 100), ("TASK-5", 200), ("TASK-2", 300)]);
        assert_eq!(
            sort_ready(ready, &p, &q, ReadyOrder::Priority),
            ids(&["TASK-7", "TASK-5", "TASK-2"])
        );
    }

    #[test]
    fn priority_still_beats_insertion_time() {
        // The default must NOT become "queue order" by the back door: a
        // high-priority spec queued last still leads the wave.
        let ready = ids(&["TASK-1", "TASK-2", "BUG-9"]);
        let p = prios(&[
            ("TASK-1", "Medium"),
            ("TASK-2", "Medium"),
            ("BUG-9", "High"),
        ]);
        let q = queued_at(&[("TASK-1", 1), ("TASK-2", 2), ("BUG-9", 999)]);
        assert_eq!(
            sort_ready(ready, &p, &q, ReadyOrder::Priority),
            ids(&["BUG-9", "TASK-1", "TASK-2"])
        );
    }

    #[test]
    fn a_spec_with_no_added_at_sorts_last_and_stays_deterministic() {
        // An unreadable/absent queue entry must not jump the wave with an
        // invented epoch position, and equal keys fall back to the display id.
        let ready = ids(&["Z-1", "A-2", "M-3"]);
        let p = prios(&[("Z-1", "Medium"), ("A-2", "Medium"), ("M-3", "Medium")]);
        let q = queued_at(&[("M-3", 7)]);
        assert_eq!(
            sort_ready(ready, &p, &q, ReadyOrder::Queue),
            ids(&["M-3", "A-2", "Z-1"])
        );
        // With NO timestamps at all the order is the id order — a total,
        // reproducible fallback rather than whatever the caller collected.
        assert_eq!(
            sort_ready(
                ids(&["Z-1", "A-2", "M-3"]),
                &p,
                &no_added_at(),
                ReadyOrder::Queue
            ),
            ids(&["A-2", "M-3", "Z-1"])
        );
    }

    #[test]
    fn unknown_or_missing_priority_ranks_with_medium() {
        let ready = ids(&["A", "B", "C", "D"]);
        // B has no entry at all; C carries a value from outside the taxonomy.
        let p = prios(&[("A", "low"), ("C", "urgent"), ("D", "high")]);
        let q = queued_at(&[("A", 1), ("B", 2), ("C", 3), ("D", 4)]);
        assert_eq!(
            sort_ready(ready, &p, &q, ReadyOrder::Priority),
            ids(&["D", "B", "C", "A"])
        );
    }

    #[test]
    fn priority_order_still_lets_serialize_groups_sequence_members() {
        // Two members of one serialize group + an unrelated spec. Priority
        // reorders the set; the downstream collapse still admits exactly one
        // group member per wave — and it is the high-priority one.
        let ready = ids(&["TASK-1", "TASK-2", "TASK-3"]);
        let p = prios(&[("TASK-1", "Low"), ("TASK-2", "Medium"), ("TASK-3", "High")]);
        let q = queued_at(&[("TASK-1", 1), ("TASK-2", 2), ("TASK-3", 3)]);
        let sorted = sort_ready(ready, &p, &q, ReadyOrder::Priority);
        let mut groups: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        groups.insert("TASK-1".to_string(), vec!["core".to_string()]);
        groups.insert("TASK-3".to_string(), vec!["core".to_string()]);
        groups.insert("TASK-2".to_string(), vec![]);
        let (kept, held) = collapse_serialize_groups(sorted, &groups);
        assert_eq!(kept, ids(&["TASK-3", "TASK-2"]));
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].spec, "TASK-1");
        assert_eq!(held[0].held_by, "TASK-3");
        assert_eq!(held[0].group, "core");
    }

    #[test]
    fn queue_order_lets_the_earliest_queued_member_claim_a_serialize_group() {
        // Under `order = queue` the group claim goes to the member queued
        // first, regardless of priority or id.
        let ready = ids(&["TASK-1", "TASK-9"]);
        let p = prios(&[("TASK-1", "High"), ("TASK-9", "Low")]);
        let q = queued_at(&[("TASK-9", 1), ("TASK-1", 2)]);
        let sorted = sort_ready(ready, &p, &q, ReadyOrder::Queue);
        let mut groups: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        groups.insert("TASK-1".to_string(), vec!["core".to_string()]);
        groups.insert("TASK-9".to_string(), vec!["core".to_string()]);
        let (kept, held) = collapse_serialize_groups(sorted, &groups);
        assert_eq!(kept, ids(&["TASK-9"]));
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].spec, "TASK-1");
    }

    #[test]
    fn ready_order_parses_config_values_and_rejects_typos() {
        assert_eq!(parse_ready_order("priority"), Some(ReadyOrder::Priority));
        assert_eq!(parse_ready_order("  QUEUE "), Some(ReadyOrder::Queue));
        // Back-compat alias — still accepted, and now truthful.
        assert_eq!(parse_ready_order("input"), Some(ReadyOrder::Queue));
        assert_eq!(parse_ready_order("prioritise"), None);
        // Dropped rather than shipped as a third mode: id order is an accident
        // of spelling, not a schedule. trace:TASK-1175
        assert_eq!(parse_ready_order("id"), None);
        assert_eq!(ReadyOrder::default(), ReadyOrder::Priority);
    }

    // STORY-544: the human-facing presentation helpers — plain-language
    // selector gloss + next-step footer pointing at the /aida-burndown skill.
    #[test]
    fn selector_summary_default_is_plain_language() {
        let s = selector_summary("approved", None, None);
        assert!(s.contains("Showing approved specs (default)."));
        // The narrowing hint names all three knobs and drops bare "selector".
        assert!(s.contains("--batch NAME"));
        assert!(s.contains("--tag X"));
        assert!(s.contains("--status <s>"));
        assert!(!s.to_lowercase().contains("selector:"));
    }

    #[test]
    fn selector_summary_reflects_filters() {
        let s = selector_summary("draft", Some("papercut"), Some("scaffolding"));
        assert!(s.contains("Showing draft specs filtered to tag papercut + batch scaffolding."));
    }

    // STORY-546: the queue-gate split.
    #[test]
    fn split_by_signoff_blesses_only_queued_pickable_specs() {
        let pickable = vec![
            "TASK-1".to_string(),
            "TASK-2".to_string(),
            "STORY-3".to_string(),
        ];
        let queued: std::collections::HashSet<String> =
            ["TASK-2".to_string()].into_iter().collect();
        let (ready, awaiting) = split_by_signoff(pickable, &queued);
        // Only the queued pickable spec is drainable.
        assert_eq!(ready, vec!["TASK-2".to_string()]);
        // The rest are pickable but await advisor sign-off (queueing).
        assert_eq!(awaiting, vec!["TASK-1".to_string(), "STORY-3".to_string()]);
    }

    #[test]
    fn split_by_signoff_empty_queue_blesses_nothing() {
        let pickable = vec!["TASK-1".to_string(), "TASK-2".to_string()];
        let (ready, awaiting) = split_by_signoff(pickable, &std::collections::HashSet::new());
        assert!(ready.is_empty());
        assert_eq!(awaiting.len(), 2);
    }

    // STORY-547: the broader "why still open" explainer.
    fn open(
        req_type: &str,
        status: &str,
        tags: &[&str],
        blocked: bool,
        decision: bool,
        in_flight: bool,
    ) -> OpenFacts {
        OpenFacts {
            id: "X-1".to_string(),
            req_type: req_type.to_string(),
            status: status.to_string(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            has_unsatisfied_blocker: blocked,
            has_pending_decision: decision,
            in_flight,
            in_flight_role: None,
            findings: Vec::new(),
            residual_notes: Vec::new(),
            epic_rollup: None,
            human_only: false,
            epic_children_in_motion: false,
        }
    }

    /// BUG-564: a human-only spec whose DERIVED bucket is NOT a needs-human
    /// bucket must still be classified human-required (so it surfaces in
    /// `aida list human`), and must NOT be human-required when the marker is
    /// off. This is the predicate the human-view filter now folds `f.human_only`
    /// into — before the fix the view fed `human_required` a hardcoded `false`,
    /// so the init scaffolding-commit task (High/Approved → Actionable, a
    /// non-human bucket) was invisible while the queue's human-only bucket showed
    /// it. trace:BUG-564 | ai:claude
    #[test]
    fn human_required_folds_in_human_only_marker_over_non_human_bucket() {
        // An approved, unblocked, decision-free, in-flight-free task classifies
        // to the Actionable bucket — explicitly NOT a needs-human bucket.
        let f = open("task", "approved", &[], false, false, false);
        let (bucket, _) = explain_open(&f);
        assert!(
            !bucket.needs_human(),
            "precondition: {bucket:?} must be a non-human bucket for this test"
        );

        // human_only OFF → not human-required (the bucket alone doesn't qualify).
        assert!(
            !human_required(&f, false),
            "non-human bucket without the human_only marker must NOT be human-required"
        );

        // human_only ON → human-required PURELY via the orthogonal marker, even
        // though the derived bucket self-resolves. This is the BUG-564 case.
        assert!(
            human_required(&f, true),
            "a human-only spec must be human-required even with a non-human bucket"
        );
    }

    /// BUG-511: a live reviewer lease reads "being reviewed", any other
    /// (or unknown) role keeps the generic in-flight line — and both stay
    /// in the InFlight bucket so the footer says "nothing to do".
    #[test]
    fn explain_open_in_flight_names_review_when_role_is_reviewer() {
        let mut f = open("task", "approved", &[], false, false, true);
        f.in_flight_role = Some("reviewer".to_string());
        let (bucket, reason) = explain_open(&f);
        assert_eq!(bucket, OpenBucket::InFlight);
        assert!(reason.contains("being reviewed"), "{reason}");

        f.in_flight_role = Some("implementer".to_string());
        let (bucket, reason) = explain_open(&f);
        assert_eq!(bucket, OpenBucket::InFlight);
        assert!(!reason.contains("being reviewed"), "{reason}");

        f.in_flight_role = None;
        let (bucket, reason) = explain_open(&f);
        assert_eq!(bucket, OpenBucket::InFlight);
        assert!(reason.contains("in flight"), "{reason}");
    }

    #[test]
    fn explain_open_buckets_every_status_and_signal() {
        // Live + held + decision signals win over everything else.
        assert_eq!(
            explain_open(&open("task", "approved", &[], false, false, true)).0,
            OpenBucket::InFlight
        );
        assert_eq!(
            explain_open(&open(
                "task",
                "approved",
                &["review:draft-only"],
                false,
                false,
                false
            ))
            .0,
            OpenBucket::HeldForReview
        );
        assert_eq!(
            explain_open(&open("task", "approved", &[], false, true, false)).0,
            OpenBucket::AwaitingDecision
        );
        assert_eq!(
            explain_open(&open(
                "bug",
                "approved",
                &["needs-design-signoff"],
                false,
                false,
                false
            ))
            .0,
            OpenBucket::AwaitingDecision
        );
        assert_eq!(
            explain_open(&open(
                "task",
                "draft",
                &["deferred:post-stability"],
                false,
                false,
                false
            ))
            .0,
            OpenBucket::Deferred
        );
        assert_eq!(
            explain_open(&open("story", "approved", &[], true, false, false)).0,
            OpenBucket::Blocked
        );
        // Structural facts.
        assert_eq!(
            explain_open(&open("epic", "planned", &[], false, false, false)).0,
            OpenBucket::Umbrella
        );
        assert_eq!(
            explain_open(&open("vision", "inprogress", &[], false, false, false)).0,
            OpenBucket::LongLived
        );
        // Status fallbacks.
        assert_eq!(
            explain_open(&open("task", "needsattention", &[], false, false, false)).0,
            OpenBucket::AwaitingDecision
        );
        assert_eq!(
            explain_open(&open("task", "draft", &[], false, false, false)).0,
            OpenBucket::Ungroomed
        );
        assert_eq!(
            explain_open(&open("task", "done", &[], false, false, false)).0,
            OpenBucket::AwaitingMerge
        );
        assert_eq!(
            explain_open(&open("task", "inprogress", &[], false, false, false)).0,
            OpenBucket::InProgress
        );
        // Approved + unblocked + ungated = the burndown-ready case.
        assert_eq!(
            explain_open(&open(
                "task",
                "approved",
                &["papercut"],
                false,
                false,
                false
            ))
            .0,
            OpenBucket::Actionable
        );
    }

    // STORY-727 (FIX 6 + FIX 11): the plain-language `why` reasons NAME the next
    // command and read DIFFERENTLY for Approved vs Planned, so a human learns
    // what Planned means and never sees the old "burndown ready set" jargon.
    #[test]
    fn explain_open_reasons_name_command_and_differentiate_approved_planned() {
        let approved = explain_open(&open("task", "approved", &[], false, false, false)).1;
        let planned = explain_open(&open("task", "planned", &[], false, false, false)).1;
        let inprogress = explain_open(&open("task", "inprogress", &[], false, false, false)).1;

        // Approved names the autonomous drive and the manual alternative.
        assert!(approved.contains("aida zen"), "approved reason: {approved}");
        assert!(
            approved.contains("aida ship"),
            "approved reason: {approved}"
        );

        // Planned names the same commands but reads differently (design settled).
        assert!(planned.contains("aida zen"), "planned reason: {planned}");
        assert!(
            planned.contains("design is settled"),
            "planned reason: {planned}"
        );
        assert_ne!(
            approved, planned,
            "Approved and Planned must read differently (FIX 11)"
        );

        // In-progress says what's true and points at the liveness view, not the
        // old "work in progress" tautology.
        assert!(
            inprogress.contains("aida status"),
            "inprogress reason: {inprogress}"
        );
        assert_ne!(inprogress, "work in progress");

        // No jargon leaks: the old "burndown ready set" phrasing is gone.
        assert!(!approved.contains("burndown plan"));
        assert!(!planned.contains("burndown plan"));
    }

    // STORY-729 (FIX 7): a Done spec's `why` reason names the branch-vs-merge
    // reality — "awaiting merge" — so a human reads it (not just the colour) and
    // can tell Done from Completed by the words alone. (The Completed side is the
    // terminal-status branch in `handle_why`, covered by the CLI text there.)
    #[test]
    fn explain_open_done_reason_says_awaiting_merge() {
        let done = explain_open(&open("task", "done", &[], false, false, false)).1;
        assert!(
            done.contains("awaiting merge"),
            "done reason must say 'awaiting merge': {done}"
        );
        assert!(
            done.contains("on a branch"),
            "done reason must say it's on a branch: {done}"
        );
    }

    #[test]
    fn explain_open_precedence_decision_beats_structural() {
        // An epic that is ALSO tagged for a human decision reports the decision
        // (the more actionable, human-facing reason) rather than "umbrella".
        let (bucket, _) = explain_open(&open(
            "epic",
            "draft",
            &["needs-design-signoff"],
            false,
            false,
            false,
        ));
        assert_eq!(bucket, OpenBucket::AwaitingDecision);
    }

    // BUG-543: a fully-delivered epic (rollup completed == total) surfaces as
    // ReadyToClose with the one-click close command naming its own id; a
    // partial rollup (or no rollup) stays the generic Umbrella.
    #[test]
    fn explain_open_epic_ready_to_close_when_rollup_complete() {
        let mut f = open("epic", "draft", &[], false, false, false);
        f.id = "EPIC-41".to_string();

        // All children Completed → ready to close.
        f.epic_rollup = Some((4, 4));
        let (bucket, reason) = explain_open(&f);
        assert_eq!(bucket, OpenBucket::ReadyToClose);
        assert!(reason.contains("ready to close"), "{reason}");
        assert!(
            reason.contains("aida edit EPIC-41 --status completed"),
            "names the close command with its own id: {reason}"
        );
        assert!(
            bucket.needs_human(),
            "ready-to-close needs operator confirm"
        );

        // TASK-808: a child still open AND something IN MOTION → epic is
        // SELF-DRIVING → InProgress, and it must NOT need a human (it drops out
        // of `aida human`). TASK-884: "in motion" is now an explicit signal.
        f.epic_rollup = Some((3, 4));
        f.epic_children_in_motion = true;
        assert_eq!(explain_open(&f).0, OpenBucket::InProgress);
        assert!(
            !explain_open(&f).0.needs_human(),
            "an in-flight epic is driven by its children — not a human action"
        );

        // No rollup info (a childless epic) → needs decomposition → Umbrella
        // (still a human action: break it into children).
        f.epic_rollup = None;
        assert_eq!(explain_open(&f).0, OpenBucket::Umbrella);
        assert!(
            explain_open(&f).0.needs_human(),
            "a childless epic needs decomposition"
        );

        // A zero-total rollup (no children) is the same childless case → Umbrella.
        f.epic_rollup = Some((0, 0));
        assert_eq!(explain_open(&f).0, OpenBucket::Umbrella);
    }

    // TASK-884: a draft epic WITH children that are all still un-started (nothing
    // in motion) must surface in the advisor's `decompose` lane (Umbrella), not
    // vanish into the invisible-to-both-worklists InProgress bucket. This was the
    // dogfood gap: EPIC-6 appeared in NO advisor section.
    #[test]
    fn explain_open_draft_epic_with_unstarted_children_is_umbrella() {
        let mut f = open("epic", "draft", &[], false, false, false);
        f.id = "EPIC-6".to_string();
        // Children exist (total > 0) but none are Completed/Done/InProgress.
        f.epic_rollup = Some((0, 3));
        f.epic_children_in_motion = false;

        let (bucket, reason) = explain_open(&f);
        assert_eq!(
            bucket,
            OpenBucket::Umbrella,
            "an epic whose children are all un-started belongs in the decompose lane"
        );
        assert!(
            bucket.advisor_seat(),
            "the decompose lane is an advisor-seat bucket — it must be visible on `aida advisor`"
        );
        assert!(reason.contains("un-started"), "{reason}");

        // The moment a single child starts moving, it flips to self-driving.
        f.epic_children_in_motion = true;
        assert_eq!(explain_open(&f).0, OpenBucket::InProgress);
    }

    /// TASK-744: the split `needs-human` umbrella routes to distinct buckets —
    /// `needs-supervised-build` is keyboard-build (`--zen`), `needs-decision`
    /// is a decision to answer; bare `needs-human` stays a decision (the
    /// umbrella) until specs are migrated.
    #[test]
    fn split_needs_human_routes_build_supervised_vs_decision() {
        assert_eq!(
            explain_open(&open(
                "task",
                "approved",
                &["needs-supervised-build"],
                false,
                false,
                false
            ))
            .0,
            OpenBucket::BuildSupervised
        );
        assert_eq!(
            explain_open(&open(
                "task",
                "approved",
                &["needs-decision"],
                false,
                false,
                false
            ))
            .0,
            OpenBucket::AwaitingDecision
        );
        assert_eq!(
            explain_open(&open(
                "task",
                "approved",
                &["needs-human"],
                false,
                false,
                false
            ))
            .0,
            OpenBucket::AwaitingDecision
        );
        // The new bucket has a stable key and still demands a human (at the keyboard).
        assert_eq!(OpenBucket::BuildSupervised.key(), "build-supervised");
        assert!(OpenBucket::BuildSupervised.needs_human());
    }

    #[test]
    fn open_bucket_keys_and_needs_human_are_stable() {
        assert_eq!(OpenBucket::HeldForReview.key(), "held-for-review");
        assert_eq!(OpenBucket::Deferred.key(), "deferred");
        assert!(OpenBucket::HeldForReview.needs_human());
        assert!(OpenBucket::Ungroomed.needs_human());
        assert!(OpenBucket::Umbrella.needs_human());
        // Self-resolving / flow buckets don't demand a human nudge.
        assert!(!OpenBucket::InProgress.needs_human());
        assert!(!OpenBucket::AwaitingMerge.needs_human());
        assert!(!OpenBucket::Deferred.needs_human());
    }

    // SPIKE-57 / TASK-746: the canonical predicate must agree with
    // `OpenBucket::needs_human()` for every open-bucket signal, and must OR in
    // the orthogonal `human_only` marker.
    #[test]
    fn human_required_matches_needs_human_for_open_buckets() {
        // Buckets that need a human → predicate true (human_only = false).
        let held = open(
            "task",
            "approved",
            &["review:draft-only"],
            false,
            false,
            false,
        );
        assert!(human_required(&held, false));
        let decision = open("task", "approved", &[], false, true, false);
        assert!(human_required(&decision, false));
        let ungroomed = open("task", "draft", &[], false, false, false);
        assert!(human_required(&ungroomed, false));
        let umbrella = open("epic", "planned", &[], false, false, false);
        assert!(human_required(&umbrella, false));

        // Self-resolving / flow buckets → predicate false (no marker).
        let in_flight = open("task", "approved", &[], false, false, true);
        assert!(!human_required(&in_flight, false));
        let in_progress = open("task", "inprogress", &[], false, false, false);
        assert!(!human_required(&in_progress, false));
        let awaiting_merge = open("task", "done", &[], false, false, false);
        assert!(!human_required(&awaiting_merge, false));
    }

    #[test]
    fn human_required_ors_in_human_only_marker() {
        // A spec whose bucket would self-resolve is STILL human-required when
        // the orthogonal `human_only` marker is set.
        let in_progress = open("task", "inprogress", &[], false, false, false);
        assert!(!human_required(&in_progress, false));
        assert!(human_required(&in_progress, true));
    }

    // BUG-493: the HeldForReview reason must reconcile against real forge state
    // rather than asserting a draft PR exists purely from the `review:draft-only`
    // tag. Origin: `aida why TASK-715` claimed "held as a draft PR for human
    // review" after draft PR #709 was CLOSED in the Session-63 reset, while
    // `aida show TASK-715` correctly reported "no PR opened yet".
    #[test]
    fn reconcile_held_for_review_honest_about_pr_state() {
        // Open PR found — the tag's claim holds; name the PR number.
        let open_reason = reconcile_held_for_review(&DraftPrObservation::Open(709));
        assert!(open_reason.contains("#709"));
        assert!(open_reason.to_lowercase().contains("held"));

        // No open PR — the BUG-493 case. The reason must NOT over-claim that a
        // draft PR is held for review; it must say no open PR exists and point
        // the user at the recovery (reopen / `aida show`).
        let closed_reason = reconcile_held_for_review(&DraftPrObservation::NoOpenPr);
        let lc = closed_reason.to_lowercase();
        assert!(
            lc.contains("no open pr"),
            "closed/absent PR must be reported as no open PR, got: {closed_reason}"
        );
        // The exact over-claim BUG-493 is about must be gone: never assert the
        // draft PR IS held for review when none is open.
        assert!(
            !lc.contains("held as a draft pr for human review") && !lc.contains("held as draft pr"),
            "must not assert a held draft PR when none is open, got: {closed_reason}"
        );
        assert!(lc.contains("reopen") || lc.contains("aida show"));

        // Forge unreachable — soften, don't assert.
        let unknown_reason = reconcile_held_for_review(&DraftPrObservation::Unverifiable);
        let ulc = unknown_reason.to_lowercase();
        assert!(!ulc.contains("held as a draft pr for human review"));
        assert!(ulc.contains("could not reach") || ulc.contains("verify"));
    }

    /// TASK-741: the held-for-review reconcile enforces a NAMED cross-axis
    /// invariant declared in the lifecycle model. Pin that the row exists (so a
    /// rename/removal in `lifecycle.rs` breaks here, at the consumer) and that
    /// the model's predicate agrees with each determinable reconcile arm.
    #[test]
    fn reconcile_held_for_review_enforces_declared_invariant() {
        use aida_core::lifecycle::{held_for_review_claim_holds, invariant};
        let row = invariant("held-for-review-implies-open-draft-pr")
            .expect("held-for-review invariant is declared in lifecycle::INVARIANTS");
        assert_eq!(row.origin, "BUG-493");
        // The Open arm asserts the claim holds; the NoOpenPr arm asserts it is
        // violated — exactly what the model predicate says.
        assert!(held_for_review_claim_holds(true));
        assert!(!held_for_review_claim_holds(false));
    }

    #[test]
    fn next_step_footer_points_at_run_command_and_skill() {
        let f = next_step_footer();
        // (a) points at the real `aida burndown run` CLI command (BUG-494:
        // the footer used to falsely claim this command didn't exist).
        assert!(f.contains("aida burndown run"));
        // (b) still notes /aida-burndown as the in-Claude alternative.
        assert!(f.contains("/aida-burndown"));
        // (c) no longer denies the run command exists.
        assert!(!f.to_lowercase().contains("there is no"));
        assert!(!f.to_lowercase().contains("not a cli subcommand"));
        // No internal trace SPEC-IDs leak into user-facing text.
        assert!(!f.contains("STORY-"));
        assert!(!f.contains("BUG-"));
    }

    // STORY-565: the path-to-empty footer builder is pure — testable without a
    // store. trace:STORY-565 | ai:claude
    #[test]
    fn render_path_to_empty_is_none_for_empty_queue() {
        assert!(render_path_to_empty(&[], None).is_none());
    }

    #[test]
    fn render_path_to_empty_disambiguates_drain_from_clear() {
        let items = vec![QueuedItem::new(
            "TASK-101".to_string(),
            OpenBucket::Actionable,
        )];
        let f = render_path_to_empty(&items, None).expect("non-empty");
        assert!(f.starts_with("To empty this queue:"));
        // Ready count + BOTH the drain (does the work) and clear (drops them)
        // commands on the one line — AC2.
        assert!(f.contains("1 ready"));
        assert!(f.contains("aida burndown run"));
        assert!(f.contains("does the work"));
        assert!(f.contains("aida queue clear"));
        // No per-item "needs you" line when everything is ready.
        assert!(!f.contains("needs you"));
    }

    #[test]
    fn render_path_to_empty_names_single_action_per_nonready_item() {
        let items = vec![
            QueuedItem::new("TASK-1".to_string(), OpenBucket::Actionable),
            QueuedItem::new("STORY-2".to_string(), OpenBucket::HeldForReview),
            QueuedItem::new("TASK-3".to_string(), OpenBucket::BuildSupervised),
            QueuedItem::new("BUG-4".to_string(), OpenBucket::AwaitingDecision),
        ];
        let f = render_path_to_empty(&items, None).expect("non-empty");
        // One ready item still drives the drain/clear line.
        assert!(f.contains("1 ready"));
        // Each non-ready item gets its OWN line with the operator's id + the
        // single next action, inlining the id where the command takes one.
        assert!(f.contains("STORY-2") && f.contains("aida review STORY-2"));
        assert!(f.contains("TASK-3") && f.contains("aida queue work TASK-3 --zen"));
        assert!(f.contains("BUG-4") && f.contains("aida questions"));
        // Exactly three "needs you" lines (the three non-ready items).
        assert_eq!(f.matches("needs you").count(), 3);
        // No internal trace SPEC-IDs leak into the STATIC framing. The
        // operator's OWN queued ids (TASK-1 etc.) are fine — those aren't
        // breadcrumbs. We only assert the framing words carry no leak.
        for line in f.lines() {
            // Strip the operator's own ids by checking the static prose only.
            assert!(!line.contains("trace:"));
        }
    }

    // BUG-621: the self-resolving (in-flight / awaiting-merge / long-lived)
    // bucket must NEVER pair "needs you" with "nothing to do" — it reads as
    // requiring no operator action. trace:BUG-621 | ai:claude
    #[test]
    fn render_path_to_empty_self_resolving_is_not_needs_you() {
        for bucket in [
            OpenBucket::InFlight,
            OpenBucket::AwaitingMerge,
            OpenBucket::LongLived,
        ] {
            let items = vec![QueuedItem::new("TASK-894", bucket)];
            let f = render_path_to_empty(&items, None).expect("non-empty");
            // The self-contradiction is gone: no "needs you" on a self-resolving
            // item, and no "needs you: ... nothing to do" pairing anywhere.
            assert!(
                !f.contains("needs you"),
                "self-resolving bucket {bucket:?} must not say 'needs you': {f}"
            );
            assert!(
                !f.contains("nothing to do"),
                "self-resolving bucket {bucket:?} must not say 'nothing to do': {f}"
            );
            // It reads as no-action: "nothing for you to do; clears when shipped".
            assert!(
                f.contains("nothing for you to do") && f.contains("TASK-894"),
                "self-resolving item should read as no-action: {f}"
            );
        }
    }

    // BUG-621: an in-progress spec WITHOUT a live lease (drift) is NOT
    // self-resolving — it must not get the reassuring "clears when shipped" line,
    // and it must not say "needs you: ... nothing to do" either. It reads as
    // stalled. trace:BUG-621 | ai:claude
    #[test]
    fn render_path_to_empty_in_progress_without_lease_reads_as_stalled() {
        let items = vec![QueuedItem::new(
            "TASK-895".to_string(),
            OpenBucket::InProgress,
        )];
        let f = render_path_to_empty(&items, None).expect("non-empty");
        assert!(
            !f.contains("needs you"),
            "drift must not say 'needs you': {f}"
        );
        assert!(
            !f.contains("nothing to do"),
            "drift must not say 'nothing to do': {f}"
        );
        assert!(
            !f.contains("clears when shipped"),
            "drift must NOT promise resolution: {f}"
        );
        assert!(
            f.contains("stalled") && f.contains("TASK-895"),
            "drift should read as stalled with a nudge: {f}"
        );
    }

    // BUG-753: with a LIVE drain whose plan covers the queued specs, the footer
    // names the drain and offers the watch verb — it must NOT recommend
    // `aida burndown run` (a launch the single-drain lock would refuse) and
    // must NOT count drain-scheduled specs as ready-to-drain.
    // trace:BUG-753 | ai:claude
    #[test]
    fn render_path_to_empty_live_drain_replaces_launch_with_watch() {
        let items = vec![
            QueuedItem::new("TASK-1".to_string(), OpenBucket::Actionable),
            QueuedItem::new("BUG-2".to_string(), OpenBucket::Actionable),
        ];
        let drain = DrainFooterOverlay {
            scheduled: ["TASK-1".to_string(), "BUG-2".to_string()]
                .into_iter()
                .collect(),
            in_flight: ["TASK-9".to_string()].into_iter().collect(),
        };
        let f = render_path_to_empty(&items, Some(&drain)).expect("non-empty");
        // Names the drain + the watch verb.
        assert!(
            f.contains("a drain is already working this queue"),
            "footer must name the live drain: {f}"
        );
        assert!(f.contains("2 scheduled"), "scheduled count: {f}");
        assert!(f.contains("1 in flight"), "in-flight count: {f}");
        assert!(
            f.contains("aida drain status"),
            "footer must offer the watch verb: {f}"
        );
        // No launch recommendation, no drain-scheduled specs counted as ready.
        assert!(
            !f.contains("aida burndown run"),
            "must not recommend launching a second drain: {f}"
        );
        // ("already" contains "ready" — match the count-line shape instead.)
        assert!(!f.contains("ready    →"), "no ready-to-drain count: {f}");
    }

    // BUG-753: ready items OUTSIDE the live drain's plan still must not get the
    // `burndown run` launch recommendation (the lock is held); they read as
    // waiting for the running drain, with clear still offered.
    // trace:BUG-753 | ai:claude
    #[test]
    fn render_path_to_empty_live_drain_uncovered_ready_waits() {
        let items = vec![
            QueuedItem::new("TASK-1".to_string(), OpenBucket::Actionable),
            QueuedItem::new("BUG-2".to_string(), OpenBucket::Actionable),
        ];
        let drain = DrainFooterOverlay {
            scheduled: ["TASK-1".to_string()].into_iter().collect(),
            in_flight: Default::default(),
        };
        let f = render_path_to_empty(&items, Some(&drain)).expect("non-empty");
        assert!(f.contains("1 scheduled"), "covered spec is scheduled: {f}");
        assert!(
            f.contains("1 ready") && f.contains("waits for the running drain"),
            "uncovered ready waits for the drain: {f}"
        );
        assert!(f.contains("aida queue clear"), "clear stays offered: {f}");
        assert!(
            !f.contains("aida burndown run"),
            "must not recommend launching a second drain: {f}"
        );
    }

    // BUG-765: a spec the LIVE drain reports in-flight or scheduled must NOT
    // get the "stalled — no live session lease" pick-it-up hint the banner just
    // contradicted — following it would double-dispatch a spec mid-flight. It
    // reads as in-flight instead. trace:BUG-765 | ai:claude
    #[test]
    fn render_path_to_empty_live_drain_suppresses_stalled_hint_for_its_specs() {
        for covered_via in ["in_flight", "scheduled"] {
            let items = vec![QueuedItem::new(
                "TASK-1162".to_string(),
                OpenBucket::InProgress,
            )];
            let mut drain = DrainFooterOverlay::default();
            let set = ["TASK-1162".to_string()].into_iter().collect();
            match covered_via {
                "in_flight" => drain.in_flight = set,
                _ => drain.scheduled = set,
            }
            let f = render_path_to_empty(&items, Some(&drain)).expect("non-empty");
            assert!(
                !f.contains("stalled"),
                "drain-{covered_via} spec must not read as stalled: {f}"
            );
            assert!(
                !f.contains("aida queue work TASK-1162"),
                "no pick-it-up hint for a drain-{covered_via} spec: {f}"
            );
            assert!(
                f.contains("nothing for you to do") && f.contains("TASK-1162"),
                "drain-{covered_via} spec reads as in flight: {f}"
            );
        }
    }

    // BUG-765: a live drain does NOT blanket-suppress the hint — an in-progress
    // spec OUTSIDE the drain's plan is still genuinely stalled.
    // trace:BUG-765 | ai:claude
    #[test]
    fn render_path_to_empty_live_drain_keeps_stalled_hint_for_uncovered_specs() {
        let items = vec![QueuedItem::new(
            "TASK-895".to_string(),
            OpenBucket::InProgress,
        )];
        let drain = DrainFooterOverlay {
            scheduled: ["BUG-2".to_string()].into_iter().collect(),
            in_flight: ["TASK-9".to_string()].into_iter().collect(),
        };
        let f = render_path_to_empty(&items, Some(&drain)).expect("non-empty");
        assert!(
            f.contains("stalled") && f.contains("TASK-895"),
            "uncovered drift keeps the stalled nudge: {f}"
        );
    }

    // BUG-753: with NO drain, behavior is unchanged — the launch/clear pair.
    // trace:BUG-753 | ai:claude
    #[test]
    fn render_path_to_empty_no_drain_unchanged() {
        let items = vec![QueuedItem::new(
            "TASK-1".to_string(),
            OpenBucket::Actionable,
        )];
        let f = render_path_to_empty(&items, None).expect("non-empty");
        assert!(f.contains("1 ready"));
        assert!(f.contains("aida burndown run"));
        assert!(f.contains("aida queue clear"));
        assert!(!f.contains("a drain is already working this queue"));
    }

    // BUG-772: the blocker-outlook classifier — dead end outranks a parked
    // predecessor, which outranks the self-clearing chain.
    // trace:BUG-772 | ai:claude
    #[test]
    fn classify_blockers_ranks_dead_end_over_parked_over_clears() {
        let fact = |id: &str, status: Option<&str>| BlockerFact {
            id: id.to_string(),
            status: status.map(|s| s.to_string()),
        };
        // No edges → no outlook (a `blocked` TAG keeps the legacy phrasing).
        assert_eq!(classify_blockers(&[]), None);

        // Every predecessor is authorized work already moving → self-clearing.
        // Statuses arrive in the store's Debug casing (`InProgress`) and are
        // normalized inside.
        for status in ["Approved", "Planned", "InProgress", "In Progress", "Done"] {
            let out = classify_blockers(&[fact("TASK-1135", Some(status))]).expect("outlook");
            assert_eq!(
                out.kind,
                BlockerOutlookKind::Clears,
                "blocker status {status} should read as self-clearing"
            );
            assert_eq!(out.blockers, vec!["TASK-1135".to_string()]);
        }

        // A Draft / NeedsAttention predecessor is not cleared to start.
        for status in ["Draft", "NeedsAttention"] {
            let out = classify_blockers(&[
                fact("TASK-1135", Some("Approved")),
                fact("TASK-9", Some(status)),
            ])
            .expect("outlook");
            assert_eq!(out.kind, BlockerOutlookKind::PredecessorParked);
            // Only the deciding predecessor is named.
            assert_eq!(out.blockers, vec!["TASK-9".to_string()]);
        }

        // A Rejected predecessor, or a dangling edge, is a dead end — and
        // outranks both other kinds.
        for blocker in [fact("TASK-9", Some("Rejected")), fact("gone", None)] {
            let out = classify_blockers(&[
                fact("TASK-1135", Some("Approved")),
                fact("TASK-8", Some("Draft")),
                blocker.clone(),
            ])
            .expect("outlook");
            assert_eq!(out.kind, BlockerOutlookKind::DeadEnd);
            assert_eq!(out.blockers, vec![blocker.id.clone()]);
        }
    }

    // BUG-772: a spec blocked by a predecessor that is ITSELF queued/in-flight/
    // Approved must NOT appear in the "needs you" footer, and must never be
    // handed the `--status rejected` suggestion — that advises destroying
    // authorized chained work the drain will unblock itself.
    // trace:BUG-772 | ai:claude
    #[test]
    fn render_path_to_empty_blocked_by_live_predecessor_is_not_needs_you() {
        let items = vec![
            QueuedItem::new("TASK-1136", OpenBucket::Blocked).with_blocker(Some(BlockerOutlook {
                kind: BlockerOutlookKind::Clears,
                blockers: vec!["TASK-1135".to_string()],
            })),
        ];
        let f = render_path_to_empty(&items, None).expect("non-empty");
        assert!(
            !f.contains("needs you"),
            "a chain waiting on live work is not an operator action: {f}"
        );
        assert!(
            !f.contains("--status rejected"),
            "must never advise rejecting authorized chained work: {f}"
        );
        // It stays VISIBLE, and says what it is waiting on.
        assert!(
            f.contains("TASK-1136") && f.contains("TASK-1135"),
            "the waiting chain should still be shown with its blocker: {f}"
        );
        assert!(
            f.contains("nothing for you to do"),
            "it should read as no-action: {f}"
        );
    }

    // BUG-772: a spec blocked by a REJECTED predecessor IS the operator's call —
    // but the suggestion is unblock-or-re-scope, not a bare reject.
    // trace:BUG-772 | ai:claude
    #[test]
    fn render_path_to_empty_blocked_by_rejected_predecessor_suggests_unblock() {
        let items = vec![
            QueuedItem::new("TASK-1136", OpenBucket::Blocked).with_blocker(Some(BlockerOutlook {
                kind: BlockerOutlookKind::DeadEnd,
                blockers: vec!["TASK-1135".to_string()],
            })),
        ];
        let f = render_path_to_empty(&items, None).expect("non-empty");
        assert!(
            f.contains("needs you"),
            "a dead-end block is genuinely the operator's call: {f}"
        );
        assert!(
            f.contains("unblock or re-scope") && f.contains("TASK-1135"),
            "the suggestion names the blocker and the unblock verb: {f}"
        );
        assert!(
            !f.contains("--status rejected"),
            "even a dead end is not a bare-reject suggestion: {f}"
        );
    }

    // BUG-772: a Draft/NeedsAttention predecessor needs a person — on the
    // BLOCKER, not on the dependent. trace:BUG-772 | ai:claude
    #[test]
    fn render_path_to_empty_blocked_by_parked_predecessor_points_at_the_blocker() {
        let items = vec![
            QueuedItem::new("TASK-1136", OpenBucket::Blocked).with_blocker(Some(BlockerOutlook {
                kind: BlockerOutlookKind::PredecessorParked,
                blockers: vec!["TASK-1135".to_string()],
            })),
        ];
        let f = render_path_to_empty(&items, None).expect("non-empty");
        assert!(f.contains("needs you"), "{f}");
        assert!(
            f.contains("aida why TASK-1135"),
            "the action points at the BLOCKER: {f}"
        );
        assert!(!f.contains("--status rejected"), "{f}");
    }

    // BUG-772: a Blocked item with NO resolved outlook (the `blocked` TAG with
    // no graph edge) keeps the pre-existing phrasing — the fix narrows the
    // reject suggestion, it doesn't drop the bucket. trace:BUG-772 | ai:claude
    #[test]
    fn render_path_to_empty_blocked_without_outlook_keeps_legacy_sentence() {
        let items = vec![QueuedItem::new("TASK-1136", OpenBucket::Blocked)];
        let f = render_path_to_empty(&items, None).expect("non-empty");
        assert!(f.contains("needs you"), "{f}");
        assert!(f.contains("aida edit TASK-1136 --status rejected"), "{f}");
    }

    // BUG-772: a long chain truncates rather than running off the line.
    // trace:BUG-772 | ai:claude
    #[test]
    fn join_blockers_truncates_long_chains() {
        let ids: Vec<String> = (1..=5).map(|n| format!("TASK-{n}")).collect();
        let s = join_blockers(&ids);
        assert!(s.starts_with("TASK-1, TASK-2, TASK-3"), "{s}");
        assert!(s.ends_with("+2 more"), "{s}");
    }

    // TASK-723: multi-reason — derived + finding-link + residual note.
    // BUG-1715: a comment whose 9th BYTE falls inside a multi-byte character used
    // to panic `split_at` — "byte index 9 is not a char boundary" — which took
    // down `aida human` and the burndown's open-facts collection for the entire
    // project. One em-dash in one comment on one spec was enough. This is the
    // exact string that did it, in production.
    // trace:BUG-1715 | ai:claude
    #[test]
    fn multibyte_char_at_the_prefix_boundary_does_not_panic() {
        assert_eq!(
            parse_why_open_comment("SKETCH — awaiting independent advisor signoff"),
            None,
            "not a why-open comment, and must not panic deciding that"
        );
    }

    // Every byte offset around the 9-byte prefix, so no boundary is left untested.
    // trace:BUG-1715 | ai:claude
    #[test]
    fn a_multibyte_char_at_any_offset_near_the_boundary_is_safe() {
        for pad in 0..12 {
            let s = format!("{}—tail", "x".repeat(pad));
            assert_eq!(
                parse_why_open_comment(&s),
                None,
                "pad={pad} must be rejected without panicking: {s:?}"
            );
        }
    }

    // A short non-ASCII string must not panic either: the old length guard
    // compared BYTE lengths, so a 3-byte char could pass it and still not be a
    // valid split point.
    // trace:BUG-1715 | ai:claude
    #[test]
    fn short_multibyte_input_is_safe() {
        for s in ["—", "——", "é", "🙂🙂"] {
            assert_eq!(parse_why_open_comment(s), None, "{s:?} must not panic");
        }
    }

    // The fix must not break the thing the function is FOR.
    // trace:BUG-1715 | ai:claude
    #[test]
    fn a_real_why_open_comment_with_an_em_dash_in_the_reason_still_parses() {
        assert_eq!(
            parse_why_open_comment("why-open: blocked — upstream API is down"),
            Some("blocked — upstream API is down".to_string())
        );
    }

    #[test]
    fn parse_why_open_comment_extracts_reason_case_insensitively() {
        assert_eq!(
            parse_why_open_comment("why-open: waiting on upstream X"),
            Some("waiting on upstream X".to_string())
        );
        // Case-insensitive prefix, leading whitespace tolerated, reason trimmed.
        assert_eq!(
            parse_why_open_comment("  WHY-OPEN:   deferred pending budget  "),
            Some("deferred pending budget".to_string())
        );
        // Not a residual note.
        assert_eq!(parse_why_open_comment("a normal comment"), None);
        // Empty reason after the prefix is not a note.
        assert_eq!(parse_why_open_comment("why-open:   "), None);
    }

    #[test]
    fn explain_reasons_orders_derived_then_findings_then_residual() {
        let mut f = open("task", "approved", &[], false, false, false);
        f.findings = vec!["TASK-900".to_string()];
        f.residual_notes = vec!["waiting on upstream X".to_string()];
        let (bucket, reasons) = explain_reasons(&f);
        // The derived reason still drives the bucket.
        assert_eq!(bucket, OpenBucket::Actionable);
        assert_eq!(reasons.len(), 3);
        // Most-fundamental-first: derived → finding → residual.
        assert_eq!(reasons[0].source, ReasonSource::Derived);
        assert_eq!(reasons[1].source, ReasonSource::Finding);
        assert!(reasons[1].text.contains("TASK-900"));
        assert_eq!(reasons[2].source, ReasonSource::Residual);
        assert_eq!(reasons[2].text, "waiting on upstream X");
    }

    #[test]
    fn explain_reasons_derived_only_when_no_extras() {
        let f = open("task", "draft", &[], false, false, false);
        let (_, reasons) = explain_reasons(&f);
        assert_eq!(reasons.len(), 1);
        assert_eq!(reasons[0].source, ReasonSource::Derived);
    }

    #[test]
    fn reason_source_keys_are_stable() {
        assert_eq!(ReasonSource::Derived.key(), "derived");
        assert_eq!(ReasonSource::Finding.key(), "finding");
        assert_eq!(ReasonSource::Residual.key(), "residual");
    }

    // ---- STORY-563: `aida human unblock` classifier + prompt assembler. ----

    #[allow(clippy::too_many_arguments)]
    fn ufacts(
        id: &str,
        req_type: &str,
        status: &str,
        tags: &[&str],
        blocked: bool,
        decision: bool,
        in_flight: bool,
        queued: bool,
        has_acceptance: bool,
        implementable: bool,
    ) -> UnblockFacts {
        UnblockFacts {
            id: id.to_string(),
            title: format!("title of {id}"),
            req_type: req_type.to_string(),
            status: status.to_string(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            has_unsatisfied_blocker: blocked,
            has_pending_decision: decision,
            in_flight,
            queued,
            has_acceptance,
            implementable,
        }
    }

    #[test]
    fn unblock_ready_set_item_is_excluded() {
        // Queued, approved, unblocked, decision-free, has acceptance, bounded.
        let f = ufacts(
            "TASK-1",
            "task",
            "approved",
            &[],
            false,
            false,
            false,
            true,
            true,
            true,
        );
        assert!(f.in_burndown_ready_set());
        assert_eq!(classify_unblock(&f), None);
    }

    #[test]
    fn unblock_draft_needs_approval() {
        // Draft with acceptance — needs approval, not under-specified.
        let f = ufacts(
            "STORY-1",
            "story",
            "draft",
            &[],
            false,
            false,
            false,
            false,
            true,
            true,
        );
        assert_eq!(classify_unblock(&f), Some(UnblockClass::NeedsApproval));
    }

    #[test]
    fn unblock_approved_unqueued_is_queueable() {
        let f = ufacts(
            "TASK-2",
            "task",
            "approved",
            &[],
            false,
            false,
            false,
            false,
            true,
            true,
        );
        assert_eq!(classify_unblock(&f), Some(UnblockClass::ApprovedUnqueued));
        assert_eq!(
            UnblockClass::ApprovedUnqueued.action(),
            UnblockAction::Queue
        );
    }

    #[test]
    fn unblock_missing_acceptance_is_under_specified() {
        // Approved but no acceptance + implementable → clarify first.
        let f = ufacts(
            "TASK-3",
            "task",
            "approved",
            &[],
            false,
            false,
            false,
            false,
            false,
            true,
        );
        assert_eq!(classify_unblock(&f), Some(UnblockClass::UnderSpecified));
        assert_eq!(
            UnblockClass::UnderSpecified.action(),
            UnblockAction::Clarify
        );
    }

    #[test]
    fn unblock_non_implementable_missing_acceptance_is_not_under_specified() {
        // A vision has no acceptance and is non-implementable — not flagged
        // under-specified; falls through to approved-unqueued.
        let f = ufacts(
            "VIS-1",
            "vision",
            "approved",
            &[],
            false,
            false,
            false,
            false,
            false,
            false,
        );
        assert_eq!(classify_unblock(&f), Some(UnblockClass::ApprovedUnqueued));
    }

    #[test]
    fn unblock_decision_blocker_deferral_leave_parked() {
        let decision = ufacts(
            "S-1",
            "story",
            "approved",
            &["needs-design-signoff"],
            false,
            false,
            false,
            false,
            true,
            true,
        );
        assert_eq!(
            classify_unblock(&decision),
            Some(UnblockClass::DecisionPending)
        );

        let pending = ufacts(
            "S-2",
            "story",
            "approved",
            &[],
            false,
            true,
            false,
            false,
            true,
            true,
        );
        assert_eq!(
            classify_unblock(&pending),
            Some(UnblockClass::DecisionPending)
        );

        let blocked = ufacts(
            "T-1",
            "task",
            "approved",
            &[],
            true,
            false,
            false,
            false,
            true,
            true,
        );
        assert_eq!(classify_unblock(&blocked), Some(UnblockClass::BlockedBy));

        let deferred = ufacts(
            "T-2",
            "task",
            "approved",
            &["deferred:post-stability"],
            false,
            false,
            false,
            false,
            true,
            true,
        );
        assert_eq!(classify_unblock(&deferred), Some(UnblockClass::Deferred));

        for cls in [
            UnblockClass::DecisionPending,
            UnblockClass::BlockedBy,
            UnblockClass::Deferred,
            UnblockClass::BuildSupervised,
        ] {
            assert_eq!(cls.action(), UnblockAction::Leave);
        }
    }

    #[test]
    fn unblock_in_flight_and_in_progress_are_build_supervised() {
        let in_flight = ufacts(
            "T-3",
            "task",
            "approved",
            &[],
            false,
            false,
            true,
            true,
            true,
            true,
        );
        assert_eq!(
            classify_unblock(&in_flight),
            Some(UnblockClass::BuildSupervised)
        );

        let in_progress = ufacts(
            "T-4",
            "task",
            "inprogress",
            &[],
            false,
            false,
            false,
            false,
            true,
            true,
        );
        assert_eq!(
            classify_unblock(&in_progress),
            Some(UnblockClass::BuildSupervised)
        );
    }

    // BUG-502: a `review:draft-only` spec is BUILT work awaiting human review —
    // it must land in the REVIEW bucket, never QUEUE or CLARIFY (which would
    // re-build done work). Covers the STORY-543/TASK-715 dogfood regression.
    #[test]
    fn unblock_review_draft_only_is_held_for_review() {
        // Queued draft-only (like STORY-543, which was mis-routed to QUEUE).
        let queued_draft_only = ufacts(
            "STORY-543",
            "story",
            "approved",
            &["review:draft-only"],
            false,
            false,
            false,
            true,
            true,
            true,
        );
        assert_eq!(
            classify_unblock(&queued_draft_only),
            Some(UnblockClass::HeldForReview)
        );
        assert_eq!(
            UnblockClass::HeldForReview.action(),
            UnblockAction::Review,
            "built work is reviewed, never re-queued or clarified"
        );

        // Unqueued draft-only with NO acceptance section (like TASK-715, which
        // was mis-routed to CLARIFY-FIRST "missing acceptance"). Must NOT be
        // flagged under-specified — it's built.
        let unqueued_draft_only_no_acceptance = ufacts(
            "TASK-715",
            "task",
            "approved",
            &["review:draft-only"],
            false,
            false,
            false,
            false,
            false, // no acceptance
            true,
        );
        assert_eq!(
            classify_unblock(&unqueued_draft_only_no_acceptance),
            Some(UnblockClass::HeldForReview)
        );

        // A spec at Done is likewise built — review it, don't re-queue.
        let done = ufacts(
            "TASK-9",
            "task",
            "done",
            &[],
            false,
            false,
            false,
            false,
            true,
            true,
        );
        assert_eq!(classify_unblock(&done), Some(UnblockClass::HeldForReview));
    }

    // BUG-502: a queued spec that passes the pickability gate is GROOMED — even
    // without a `## Acceptance` section, because the gate (`aida burndown plan`)
    // does NOT require one. It must be excluded from `aida human unblock`, NOT
    // flagged "missing acceptance → clarify". Reconciles the under-specified
    // check with the pickability gate.
    #[test]
    fn unblock_queued_ready_without_acceptance_is_not_clarify() {
        let queued_ready_no_acceptance = ufacts(
            "BUG-499",
            "bug",
            "approved",
            &[],
            false,
            false,
            false,
            true,  // queued
            false, // no acceptance section
            true,  // implementable
        );
        assert!(
            queued_ready_no_acceptance.passes_pickability_gate(),
            "a bounded, unblocked, decision-free, untagged spec passes the gate"
        );
        assert!(
            queued_ready_no_acceptance.in_burndown_ready_set(),
            "queued + passes-gate = groomed, regardless of acceptance"
        );
        assert_eq!(
            classify_unblock(&queued_ready_no_acceptance),
            None,
            "groomed/ready specs are excluded — not flagged 'missing acceptance'"
        );
    }

    // BUG-502: the reconciliation must NOT swallow a genuinely under-specified,
    // NOT-yet-queued spec — that still needs clarifying before it can be queued.
    #[test]
    fn unblock_unqueued_missing_acceptance_still_clarifies() {
        let unqueued_no_acceptance = ufacts(
            "TASK-50",
            "task",
            "approved",
            &[],
            false,
            false,
            false,
            false, // NOT queued
            false, // no acceptance
            true,
        );
        assert!(!unqueued_no_acceptance.in_burndown_ready_set());
        assert_eq!(
            classify_unblock(&unqueued_no_acceptance),
            Some(UnblockClass::UnderSpecified)
        );
    }

    #[test]
    fn unblock_epic_is_decision_pending() {
        let f = ufacts(
            "EPIC-1",
            "epic",
            "approved",
            &[],
            false,
            false,
            false,
            false,
            true,
            false,
        );
        assert_eq!(classify_unblock(&f), Some(UnblockClass::DecisionPending));
    }

    #[test]
    fn unblock_class_keys_are_stable() {
        assert_eq!(UnblockClass::NeedsApproval.key(), "needs-approval");
        assert_eq!(UnblockClass::ApprovedUnqueued.key(), "approved-unqueued");
        assert_eq!(UnblockClass::UnderSpecified.key(), "under-specified");
        assert_eq!(UnblockClass::BuildSupervised.key(), "build-supervised");
        assert_eq!(UnblockClass::DecisionPending.key(), "decision-pending");
        assert_eq!(UnblockClass::Deferred.key(), "deferred");
        assert_eq!(UnblockClass::BlockedBy.key(), "blocked-by");
    }

    #[test]
    fn assemble_unblock_prompt_groups_by_action() {
        let lines = vec![
            UnblockLine {
                id: "TASK-2".to_string(),
                title: String::new(),
                class: UnblockClass::ApprovedUnqueued,
                reason: unblock_reason(UnblockClass::ApprovedUnqueued).to_string(),
            },
            UnblockLine {
                id: "TASK-3".to_string(),
                title: String::new(),
                class: UnblockClass::UnderSpecified,
                reason: unblock_reason(UnblockClass::UnderSpecified).to_string(),
            },
            UnblockLine {
                id: "T-1".to_string(),
                title: String::new(),
                class: UnblockClass::BlockedBy,
                reason: unblock_reason(UnblockClass::BlockedBy).to_string(),
            },
            // BUG-502: a built-and-held spec must render under REVIEW.
            UnblockLine {
                id: "STORY-543".to_string(),
                title: String::new(),
                class: UnblockClass::HeldForReview,
                reason: unblock_reason(UnblockClass::HeldForReview).to_string(),
            },
        ];
        let prompt = assemble_unblock_prompt(&lines);
        assert!(prompt.contains("QUEUE"));
        assert!(prompt.contains("CLARIFY FIRST"));
        assert!(prompt.contains("REVIEW"));
        assert!(prompt.contains("LEAVE PARKED"));
        // Each spec lands under its action heading, with its reason.
        assert!(prompt.contains("TASK-2 — approved"));
        assert!(prompt.contains("TASK-3 — missing acceptance"));
        assert!(prompt.contains("T-1 — blocked"));
        // BUG-502: the draft-only spec lands under REVIEW, not QUEUE/CLARIFY.
        assert!(prompt.contains("STORY-543 — built"));
        // The leading instruction names the actions, including review.
        assert!(prompt.contains("queue the autonomous-able"));
        assert!(prompt.contains("REVIEW the built-and-held"));
    }

    #[test]
    fn assemble_unblock_prompt_renders_empty_sections() {
        let prompt = assemble_unblock_prompt(&[]);
        assert!(prompt.contains("QUEUE: (none)"));
        assert!(prompt.contains("CLARIFY FIRST: (none)"));
        assert!(prompt.contains("REVIEW: (none)"));
        assert!(prompt.contains("LEAVE PARKED: (none)"));
    }

    // STORY-566: the pure bucket → advance-action routing the
    // `aida queue advance` router dispatches on. Every variant must map.
    #[test]
    fn advance_action_maps_every_bucket() {
        use OpenBucket::*;
        let cases = [
            (HeldForReview, AdvanceAction::Review),
            (BuildSupervised, AdvanceAction::SupervisedBuild),
            (AwaitingDecision, AdvanceAction::Decision),
            (Actionable, AdvanceAction::Drain),
            (Ungroomed, AdvanceAction::Approve),
            (Deferred, AdvanceAction::Reject),
            (Blocked, AdvanceAction::Reject),
            (ReadyToClose, AdvanceAction::Close),
            (LongLived, AdvanceAction::None),
            (Umbrella, AdvanceAction::None),
            (InFlight, AdvanceAction::None),
            (AwaitingMerge, AdvanceAction::None),
            (InProgress, AdvanceAction::None),
        ];
        for (bucket, want) in cases {
            assert_eq!(advance_action(bucket), want, "bucket {bucket:?}");
            // The label is non-empty for every bucket (menu safety).
            assert!(
                !advance_action_label(bucket).is_empty(),
                "bucket {bucket:?}"
            );
        }
    }

    // STORY-566: only the two unambiguous autonomous steps may be auto-taken
    // under `--yes`; everything human-required is skipped.
    #[test]
    fn advance_only_drain_and_approve_are_autonomous() {
        assert!(AdvanceAction::Drain.is_autonomous());
        assert!(AdvanceAction::Approve.is_autonomous());
        for a in [
            AdvanceAction::Review,
            AdvanceAction::SupervisedBuild,
            AdvanceAction::Decision,
            AdvanceAction::Reject,
            AdvanceAction::Close,
            AdvanceAction::None,
        ] {
            assert!(
                !a.is_autonomous(),
                "{a:?} must NOT be auto-taken under --yes"
            );
        }
    }

    /// STORY-614: the wave-builder must never fan more than one spec per
    /// `serialize:<group>` into one wave. Given 3 ready specs where 2 share
    /// `serialize:docs`, the kept set has exactly one of the two + the
    /// independent one (2 total); the held one is preserved (not lost) so it
    /// drains in a later wave.
    #[test]
    fn collapse_serialize_groups_holds_all_but_one_per_group() {
        let mut groups: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        groups.insert("TASK-1".to_string(), vec!["docs".to_string()]);
        groups.insert("TASK-2".to_string(), vec!["docs".to_string()]);
        groups.insert("TASK-3".to_string(), vec![]); // independent — no serialize tag

        let ready = vec![
            "TASK-1".to_string(),
            "TASK-2".to_string(),
            "TASK-3".to_string(),
        ];
        let (kept, held) = collapse_serialize_groups(ready, &groups);

        // Exactly one of the docs-group pair + the independent spec survive.
        assert_eq!(kept, vec!["TASK-1".to_string(), "TASK-3".to_string()]);
        // The held member is preserved, not dropped — and names its claimer.
        assert_eq!(
            held,
            vec![SerializeHold {
                spec: "TASK-2".to_string(),
                held_by: "TASK-1".to_string(),
                group: "docs".to_string(),
            }]
        );
        // Nothing is lost across the split.
        assert_eq!(kept.len() + held.len(), 3);
    }

    /// TASK-149: a serialize-held spec is a DISTINCT state from "awaiting
    /// advisor sign-off" — it is queued and blessed, only deferred to a later
    /// wave. With 3 queued members of one group, exactly one lands in `ready`
    /// and the other two come back as holds naming the claiming spec + group;
    /// the sign-off split (the source of `awaiting_signoff`) never sees them,
    /// so that bucket stays empty for these specs.
    // trace:TASK-149 | ai:claude
    #[test]
    fn serialize_held_members_are_not_awaiting_signoff() {
        let pickable = vec![
            "TASK-1".to_string(),
            "TASK-2".to_string(),
            "TASK-3".to_string(),
        ];
        // All three are queued — i.e. all three ARE advisor-blessed.
        let queued: std::collections::HashSet<String> = pickable.iter().cloned().collect();
        let (mut ready, awaiting) = split_by_signoff(pickable, &queued);
        assert!(
            awaiting.is_empty(),
            "every member is queued, so nothing awaits sign-off"
        );

        let mut groups: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for id in ["TASK-1", "TASK-2", "TASK-3"] {
            groups.insert(id.to_string(), vec!["docs".to_string()]);
        }
        ready.sort();
        let (kept, held) = collapse_serialize_groups(ready, &groups);

        // Exactly one member drains this wave.
        assert_eq!(kept, vec!["TASK-1".to_string()]);
        // The rest are serialize-held, each naming the claiming spec + group —
        // NOT folded into the awaiting-sign-off bucket.
        assert_eq!(
            held,
            vec![
                SerializeHold {
                    spec: "TASK-2".to_string(),
                    held_by: "TASK-1".to_string(),
                    group: "docs".to_string(),
                },
                SerializeHold {
                    spec: "TASK-3".to_string(),
                    held_by: "TASK-1".to_string(),
                    group: "docs".to_string(),
                },
            ]
        );
        assert!(
            awaiting.is_empty(),
            "serialize-held specs must never land in awaiting_signoff"
        );
    }

    /// STORY-614: specs with NO serialize tag all stay parallel — the collapse
    /// is a no-op for the untagged set.
    #[test]
    fn collapse_serialize_groups_keeps_all_untagged() {
        let groups: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        let ready = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let (kept, held) = collapse_serialize_groups(ready.clone(), &groups);
        assert_eq!(kept, ready);
        assert!(held.is_empty());
    }

    /// STORY-614: distinct groups don't collide — one spec per DISTINCT group
    /// survives, not one spec overall.
    #[test]
    fn collapse_serialize_groups_independent_groups_coexist() {
        let mut groups: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        groups.insert("X".to_string(), vec!["docs".to_string()]);
        groups.insert("Y".to_string(), vec!["core".to_string()]);
        let ready = vec!["X".to_string(), "Y".to_string()];
        let (kept, held) = collapse_serialize_groups(ready, &groups);
        assert_eq!(kept, vec!["X".to_string(), "Y".to_string()]);
        assert!(held.is_empty());
    }

    /// STORY-614: the tag parser lowercases the group and strips the prefix.
    #[test]
    fn serialize_groups_parses_prefix_case_insensitive() {
        let tags = vec![
            "serialize:Docs".to_string(),
            "batch:foo".to_string(),
            "Serialize:CORE".to_string(),
            "serialize:".to_string(), // empty group — ignored
        ];
        let mut g = serialize_groups(&tags);
        g.sort();
        assert_eq!(g, vec!["core".to_string(), "docs".to_string()]);
    }
