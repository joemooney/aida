# STORY-1687 release validator: round-4 field inventory

<!-- trace:STORY-1687 | ai:codex -->

This inventory covers the shared production validator, `open_change_head`,
`eligible`, `sensitive`, and the two returned-head guards in `release_with`.
The full hold comparison recursively includes every field shown below. Each
field fixture changes only that field. Schema/PR field fixtures inject the raw
record so the real reader's prior Unknown normalization cannot mask a dropped
comparison; ordinary hold/absorption fixtures retain the real reader.

B1 tests Supervision, Decision, Recusal and Unknown absorbed under a Rework
primary before eligibility, between eligibility and clearance, and after audit.
B2 changes every requested approval/grant field before clearance and after audit.
B3 changes ADR-75 authority tags, title, Guided/Operator mode, keystone type/tag,
covered membership and identity resolution at both release-time passes.

Pre-clearance refusals assert the refusal, byte-identical marker, unchanged
labels, absent clearance and absent comments. Post-audit refusals retain the
marker and labels and the existing authorization/comment, except a real re-arm
invalidates clearance through the existing hold writer. Every release-effect
assertion precedes read counters. The post-audit validator cannot undo the
already-written audit; it prevents mirroring/removing the hold.

Mutations run in isolated copies of the exact production source with the same
behavioural fixture, compiled in the private Cargo target with two jobs. Each
copy contains one mutation; its named witness is invoked directly. No mutant
or test bypass is retained in production. The source is restored after each
batch. A killed mutation must fail on eligibility/refusal or release state,
not a read counter.

| Compared field or condition | Mutation | Killing behavioural test | Result |
|---|---|---|---|
| M000: Full absorbed composition at release, including Supervision, Decision, Recusal and Unknown | Compare only primary `reason_kind` and `detail` | `auto_release_non_rework_absorption_at_each_phase_preserves_release_effects` | KILLED (101) |
| M001: Exact hold equality: `schema_version` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_hold_record_field_change_preserves_release_effects` | KILLED (101) |
| M002: Exact hold equality: `pr` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_hold_record_field_change_preserves_release_effects` | KILLED (101) |
| M003: Exact hold equality: `reason_kind` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_hold_record_field_change_preserves_release_effects` | KILLED (101) |
| M004: Exact hold equality: `detail` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_hold_record_field_change_preserves_release_effects` | KILLED (101) |
| M005: Exact hold equality: `recused_principals` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_hold_record_field_change_preserves_release_effects` | KILLED (101) |
| M006: Exact hold equality: `routed_to` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_hold_record_field_change_preserves_release_effects` | KILLED (101) |
| M007: Exact hold equality: `routing_state` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_hold_record_field_change_preserves_release_effects` | KILLED (101) |
| M008: Exact hold equality: `target_head_sha` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_hold_record_field_change_preserves_release_effects` | KILLED (101) |
| M009: Exact hold equality: `label_state` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_hold_record_field_change_preserves_release_effects` | KILLED (101) |
| M010: Exact hold equality: `legacy` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_hold_record_field_change_preserves_release_effects` | KILLED (101) |
| M011: Exact hold equality: `verdict_ref` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_hold_record_field_change_preserves_release_effects` | KILLED (101) |
| M012: Exact hold equality: `release_condition` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_hold_record_field_change_preserves_release_effects` | KILLED (101) |
| M013: Exact hold equality: `spec` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_hold_record_field_change_preserves_release_effects` | KILLED (101) |
| M014: Exact hold equality: `placed_by` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_hold_record_field_change_preserves_release_effects` | KILLED (101) |
| M015: Exact hold equality: `absorbed` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_hold_record_field_change_preserves_release_effects` | KILLED (101) |
| M016: Exact hold equality: `/absorbed/0/reason_kind` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_nested_hold_field_change_preserves_release_effects` | KILLED (101) |
| M017: Exact hold equality: `/absorbed/0/detail` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_nested_hold_field_change_preserves_release_effects` | KILLED (101) |
| M018: Exact hold equality: `/absorbed/0/recused_principals` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_nested_hold_field_change_preserves_release_effects` | KILLED (101) |
| M019: Exact hold equality: `/absorbed/0/target_head_sha` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_nested_hold_field_change_preserves_release_effects` | KILLED (101) |
| M020: Exact hold equality: `/absorbed/0/spec` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_nested_hold_field_change_preserves_release_effects` | KILLED (101) |
| M021: Exact hold equality: `/absorbed/0/placed_by` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_nested_hold_field_change_preserves_release_effects` | KILLED (101) |
| M022: Exact hold equality: `/absorbed/0/verdict_ref` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_nested_hold_field_change_preserves_release_effects` | KILLED (101) |
| M023: Exact hold equality: `/absorbed/0/recused_principals/0/principal_kind` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_nested_hold_field_change_preserves_release_effects` | KILLED (101) |
| M024: Exact hold equality: `/absorbed/0/recused_principals/0/principal_id` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_nested_hold_field_change_preserves_release_effects` | KILLED (101) |
| M025: Exact hold equality: `/absorbed/0/recused_principals/0/identity_status` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_nested_hold_field_change_preserves_release_effects` | KILLED (101) |
| M026: Exact hold equality: `/absorbed/0/verdict_ref/key` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_nested_hold_field_change_preserves_release_effects` | KILLED (101) |
| M027: Exact hold equality: `/absorbed/0/verdict_ref/pr` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_nested_hold_field_change_preserves_release_effects` | KILLED (101) |
| M028: Exact hold equality: `/absorbed/0/verdict_ref/reviewed_sha` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_nested_hold_field_change_preserves_release_effects` | KILLED (101) |
| M029: Exact hold equality: `/absorbed/0/verdict_ref/recorded_by` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_nested_hold_field_change_preserves_release_effects` | KILLED (101) |
| M030: Exact hold equality: `/verdict_ref/key` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_nested_hold_field_change_preserves_release_effects` | KILLED (101) |
| M031: Exact hold equality: `/verdict_ref/pr` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_nested_hold_field_change_preserves_release_effects` | KILLED (101) |
| M032: Exact hold equality: `/verdict_ref/reviewed_sha` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_nested_hold_field_change_preserves_release_effects` | KILLED (101) |
| M033: Exact hold equality: `/verdict_ref/recorded_by` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_nested_hold_field_change_preserves_release_effects` | KILLED (101) |
| M034: Exact hold equality: `/recused_principals/0/principal_kind` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_nested_hold_field_change_preserves_release_effects` | KILLED (101) |
| M035: Exact hold equality: `/recused_principals/0/principal_id` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_nested_hold_field_change_preserves_release_effects` | KILLED (101) |
| M036: Exact hold equality: `/recused_principals/0/identity_status` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_nested_hold_field_change_preserves_release_effects` | KILLED (101) |
| M037: Exact hold equality: `/routed_to/0/principal_kind` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_nested_hold_field_change_preserves_release_effects` | KILLED (101) |
| M038: Exact hold equality: `/routed_to/0/principal_id` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_nested_hold_field_change_preserves_release_effects` | KILLED (101) |
| M039: Exact hold equality: `/routed_to/0/identity_status` | Normalize just this fresh field to its pinned value before equality | `auto_release_every_nested_hold_field_change_preserves_release_effects` | KILLED (101) |
| M040: `hold.legacy == false` | Replace `!hold.legacy` with `true` | `every_non_rework_kind_primary_or_absorbed_requires_human` | KILLED (101) |
| M041: `hold.reason_kind == Rework` | Replace primary Rework comparison with `true` | `every_non_rework_kind_primary_or_absorbed_requires_human` | KILLED (101) |
| M042: Every `hold.absorbed[*].reason_kind == Rework` | Replace absorbed `.all(...)` predicate with `true` | `auto_release_non_rework_absorption_at_each_phase_preserves_release_effects` | KILLED (101) |
| M043: Fresh forge read succeeds; missing/error responses refuse | Use pinned `head.clone()` instead of reading the forge | `auto_release_refetches_moved_forge_head_before_any_release_effect` | KILLED (101) |
| M044: Forge change exists | Substitute an eligible Open response for `None` | `auto_release_missing_forge_change_preserves_every_release_effect` | KILLED (101) |
| M045: Forge `change.state == Open` | Replace Open-state comparison with `true` | `auto_release_nonopen_forge_change_preserves_every_release_effect` | KILLED (101) |
| M046: Forge `change.head_sha` exists | Substitute the eligible SHA for a missing forge head | `auto_release_missing_forge_head_preserves_every_release_effect` | KILLED (101) |
| M047: Returned live head equals pinned `verdict.reviewed_sha` before clearance (also checked after audit) | Invoke validator but discard its first returned-head comparison | `auto_release_refetches_moved_forge_head_before_any_release_effect` | KILLED (101) |
| M048: Live hold exists and exactly equals pinned hold; missing/error reads refuse | Use `Some(hold)` instead of reading and comparing the live hold | `auto_release_rereads_stricter_hold_before_any_release_effect` | KILLED (101) |
| M049: Fresh approval read and eligibility check succeed | Use pinned `verdict` instead of reading/checking approval | `auto_release_rereads_rewritten_approval_before_any_release_effect` | KILLED (101) |
| M050: Revalidated VerdictRef equality: `key`, `pr`, `reviewed_sha`, `recorded_by` | Revalidate approval, then discard VerdictRef equality | `auto_release_rechecks_changed_approval_identity_before_any_release_effect` | KILLED (101) |
| M051: Fresh covered IDs and canonical inventory read succeed on both validation passes | Use pinned IDs/store instead of re-reading them | `auto_release_rereads_spec_metadata_and_execution_mode_at_each_release_phase` | KILLED (101) |
| M052: Covered ID matches requirement `spec_id` ignoring ASCII case | Replace `spec_id` equality predicate with `true` | `auto_release_rereads_spec_metadata_and_execution_mode_at_each_release_phase` | KILLED (101) |
| M053: Covered ID matches requirement `agreed_id` ignoring ASCII case | Replace `agreed_id` equality predicate with `true` | `auto_release_rereads_spec_metadata_and_execution_mode_at_each_release_phase` | KILLED (101) |
| M054: At least one covered spec | Replace nonempty-spec guard with `true` | `auto_release_rereads_spec_metadata_and_execution_mode_at_each_release_phase` | KILLED (101) |
| M055: Every covered ID resolves to a readable requirement | Skip a missing/unresolved covered spec | `auto_release_rereads_spec_metadata_and_execution_mode_at_each_release_phase` | KILLED (101) |
| M056: Every covered requirement returns no sensitivity reason | Treat every `sensitive(req)` result as `None` | `auto_release_rereads_spec_metadata_and_execution_mode_at_each_release_phase` | KILLED (101) |
| M057: Covered `execution_mode` excludes Guided | Remove Guided from the mode exclusion | `auto_release_rereads_spec_metadata_and_execution_mode_at_each_release_phase` | KILLED (101) |
| M058: Covered `execution_mode` excludes Operator | Remove Operator from the mode exclusion | `auto_release_rereads_spec_metadata_and_execution_mode_at_each_release_phase` | KILLED (101) |
| M059: Covered `req_type` and `tags` exclude shared keystone class | Bypass shared keystone-class predicate | `auto_release_rereads_spec_metadata_and_execution_mode_at_each_release_phase` | KILLED (101) |
| M060: Covered `title` scanned for sensitive terms | Scan tags only | `auto_release_rereads_spec_metadata_and_execution_mode_at_each_release_phase` | KILLED (101) |
| M061: All covered `tags` scanned for sensitive terms | Scan title only | `auto_release_rereads_spec_metadata_and_execution_mode_at_each_release_phase` | KILLED (101) |
| M062: Title/tag sensitivity matching is ASCII-case-insensitive | Remove lowercase normalization | `each_sensitive_metadata_trigger_requires_human` | KILLED (101) |
| M063: Title/tag sensitivity term `sandbox` | Replace just this term with a nonmatching term | `each_sensitive_metadata_trigger_requires_human` | KILLED (101) |
| M064: Title/tag sensitivity term `contained` | Replace just this term with a nonmatching term | `each_sensitive_metadata_trigger_requires_human` | KILLED (101) |
| M065: Title/tag sensitivity term `confinement` | Replace just this term with a nonmatching term | `each_sensitive_metadata_trigger_requires_human` | KILLED (101) |
| M066: Title/tag sensitivity term `security` | Replace just this term with a nonmatching term | `each_sensitive_metadata_trigger_requires_human` | KILLED (101) |
| M067: Title/tag sensitivity term `authority` | Replace just this term with a nonmatching term | `each_sensitive_metadata_trigger_requires_human` | KILLED (101) |
| M068: Title/tag sensitivity term `grant` | Replace just this term with a nonmatching term | `each_sensitive_metadata_trigger_requires_human` | KILLED (101) |
| M069: Title/tag sensitivity term `identity` | Replace just this term with a nonmatching term | `each_sensitive_metadata_trigger_requires_human` | KILLED (101) |
| M070: Title/tag sensitivity term `credential` | Replace just this term with a nonmatching term | `each_sensitive_metadata_trigger_requires_human` | KILLED (101) |
| M071: Title/tag sensitivity term `secret` | Replace just this term with a nonmatching term | `each_sensitive_metadata_trigger_requires_human` | KILLED (101) |
| M072: Fresh approval `verdict` parses as Approved | Accept every parsed verdict except Unknown | `auto_release_rereads_rewritten_approval_before_any_release_effect` | KILLED (101) |
| M073: Pinned head is exactly 40 characters | Replace `head.len() == 40` with `true` | `malformed_matching_heads_cannot_authorize_release` | KILLED (101) |
| M074: Pinned head is hexadecimal | Replace hex-SHA predicate with `true` | `malformed_matching_heads_cannot_authorize_release` | KILLED (101) |
| M075: Fresh approval `reviewed_sha` is a string and equals pinned current head | Replace `sha == head` with `true` | `auto_release_changed_reviewed_sha_stays_held` | KILLED (101) |
| M076: Approval `release_grant.id` equals fresh `grant.id` | Replace just this identity-field equality with `true` | `auto_release_changed_release_grant_id_stays_held` | KILLED (101) |
| M077: Approval `release_grant.subject` equals fresh `grant.subject` | Replace just this identity-field equality with `true` | `auto_release_changed_release_grant_subject_stays_held` | KILLED (101) |
| M078: Approval `release_grant.session_id` equals fresh `grant.session_id` | Replace just this identity-field equality with `true` | `auto_release_changed_release_grant_session_stays_held` | KILLED (101) |
| M079: Approval `release_grant.recorded_by` equals approval `recorded_by` | Replace just this identity-field equality with `true` | `auto_release_changed_release_grant_recorder_stays_held` | KILLED (101) |
| M080: Approval `recorded_by` exists, is a string, and is nonblank | Remove recording-identity nonempty filter | `matching_blank_recording_identity_cannot_authorize_release` | KILLED (101) |
| M081: Fresh authority verdict is Valid; Rejected refuses (expiry, revocation, subject, roster and parent validation delegated to `grant_check_for`) | Skip fresh `grant_check`, substitute a matching Valid grant | `auto_release_rechecks_revoked_grant_before_any_release_effect` | KILLED (101) |
| M082: Fresh `grant.seat` excludes implementer | Replace implementer-seat exclusion with `true` | `auto_release_changed_fresh_grant_seat_stays_held` | KILLED (101) |
| M083: Fresh `grant.session_id` equals approval stamp session | Replace just this identity-field equality with `true` | `auto_release_changed_fresh_grant_session_stays_held` | KILLED (101) |
| M084: Fresh `grant.id` equals approval stamp ID | Replace just this identity-field equality with `true` | `auto_release_changed_fresh_grant_id_stays_held` | KILLED (101) |
| M085: Fresh `grant.subject` equals approval stamp subject | Replace just this identity-field equality with `true` | `auto_release_changed_fresh_grant_subject_stays_held` | KILLED (101) |
| M086: Fresh approval `recorded_at` determines approval time | Use the eligible pinned approval timestamp | `auto_release_changed_recorded_at_stays_held` | KILLED (101) |
| M087: Fresh approval `rounds` determines retained provenance | Use eligible pinned rounds instead of the fresh array | `auto_release_changed_rounds_stays_held` | KILLED (101) |
| M088: Each retained `round.verdict` is recognized; Unknown refuses | Replace Unknown-verdict exclusion with `true` | `auto_release_changed_approval_provenance_stays_held` | KILLED (101) |
| M089: Each blocking `round.recorded_at` determines refusal time | Use the eligible pinned refusal timestamp | `auto_release_changed_rounds_stays_held` | KILLED (101) |
| M090: Approval `recorded_at` is strictly later than every blocking round timestamp | Replace `approved_at > at` with `true` | `auto_release_changed_recorded_at_stays_held` | KILLED (101) |
| M091: At least one retained RequestChanges/Rejected round | Replace `armed` guard with `true` | `auto_release_changed_approval_provenance_stays_held` | KILLED (101) |
| M092: Current-head corpus gate is Clear for PR and all freshly covered spec keys | Treat the current-head corpus verdict as Clear | `auto_release_other_covered_spec_corpus_refusal_preserves_every_release_effect` | KILLED (101) |
| M093: Fresh verdict-corpus read succeeds | Skip corpus read, use an empty corpus | `auto_release_pr_corpus_refusal_preserves_every_release_effect` | KILLED (101) |
| M094: approval release_grant presence | Use pinned `verdict` instead of reading/checking approval | `auto_release_missing_release_grant_stays_held` | KILLED (101) |
| M095: approval recorded_at presence/RFC3339 | Use the eligible pinned approval timestamp | `auto_release_changed_approval_provenance_stays_held` | KILLED (101) |
| M096: approval rounds presence/array | Use eligible pinned rounds instead of the fresh array | `auto_release_missing_rounds_stays_held` | KILLED (101) |
| M097: round recorded_at presence/RFC3339 | Use the eligible pinned refusal timestamp | `auto_release_changed_approval_provenance_stays_held` | KILLED (101) |
| M098: fresh grant Absent | Skip fresh `grant_check`, substitute a matching Valid grant | `auto_release_absent_fresh_grant_stays_held` | KILLED (101) |
| M099: strict timestamp boundary | Replace `approved_at > at` with `true` | `approval_must_be_strictly_newer_than_every_arming_refusal` | KILLED (101) |
| M100: Validator runs before writing clearance | Drop validation before clearance | `auto_release_refetches_moved_forge_head_before_any_release_effect` | KILLED (101) |
| M101: Validator runs again after audit, before mirroring labels and removing marker | Drop validation before labels/marker removal | `auto_release_after_audit_rechecks_every_condition_before_mirroring_release` | KILLED (101) |
| M102: Every RequestChanges/Rejected round contributes to the arming-time check | Ignore Rejected rounds when finding blocking refusals | `approval_must_be_strictly_newer_than_every_arming_refusal` | KILLED (101) |
| M103: Approval and refusal instants must not be equal, including equivalent timezone offsets | Replace `>` with `>=` | `approval_must_be_strictly_newer_than_every_arming_refusal` | KILLED (101) |
| M104: Sensitive terms are matched anywhere in title/tag text | Replace substring matching with prefix matching | `each_sensitive_metadata_trigger_requires_human` | KILLED (101) |
| M105: Each retained round has a parseable string `verdict` | Default a missing round verdict to Approved | `auto_release_missing_round_verdict_stays_held` | KILLED (101) |
| M106: Every covered ID resolves to exactly one canonical requirement at eligibility and both release-time reads | Replace unique-match resolution with the old first-match `find` | `auto_release_exactly_one_requirement_per_id_at_each_phase` | KILLED (101) |
| M107: Corpus read uses the freshly re-read covered IDs before clearance and after audit | Pass initial covered IDs to `read_corpus` | `auto_release_fresh_covered_id_corpus_refusal_at_each_release_phase` | KILLED (101) |

The VerdictRef equality row explicitly includes all four fields. `key` and
`pr` are derived from the same immutable pinned hold; `reviewed_sha` is derived
from the same pinned head after the separate SHA equality check. `recorded_by`
is the independently variable field of that reconstructed reference. The full
equality bypass is killed by changing the otherwise-eligible recorder. These
derived fields do not have independent fresh inputs.

The grant row is a dependency boundary: `grant_check_for` validates expiry,
revocation, subject, roster and parent delegation. This validator compares its
Valid/Absent/Rejected result plus the returned id, subject, session and seat;
it does not compare grant principal, timestamps or delegation fields itself.
Likewise the existing `is_keystone_class` helper classifies type/tags, and
`corpus_merge_gate` classifies the fresh corpus. Their internal policy is not
changed here. `expired_revoked_subject_roster_and_parent_grants_stay_held`
retains direct coverage of the delegated rejection outcomes.

The approval stamp must exist and supply string id/subject before the grant
read; the identity comparisons then require matching id/subject/session and
recorder. Timestamp presence and RFC3339 validity and rounds-array shape are
covered by the provenance witnesses. Blocking rounds contribute `verdict` and
`recorded_at`; their `reviewed_sha` and `recorded_by` are not compared. Forge
`merged` and `hold_label` are not compared by this validator: `state` and
`head_sha` control release. `hold.verdict_ref` must exist to select the arming
key initially, and its entire structure is included in the live hold equality.

Read errors for head, hold, approval, covered inventory and corpus retain all
release effects in `auto_release_failed_release_time_reads_preserve_every_release_effect`.
Clearance deduplication (`cleared_by`, `released_by_verdict`,
`audit_comment_posted`) is retry bookkeeping after validation, not an authority
exemption; `failed_comment_or_label_keeps_hold_and_label_retry_does_not_repeat_comment`
and `failed_clearance_write_keeps_hold` cover the unchanged write/retry seams.
