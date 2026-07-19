use super::*;

// TASK-964: no `--fields` => the minimal default schema; an explicit list is
// validated + ordered; an unknown field is rejected; the `req_type` alias
// folds to `type`. trace:TASK-964
#[test]
fn toon_list_fields_default_and_selection() {
    assert_eq!(
        toon_list_fields(None).unwrap(),
        vec!["id", "title", "status", "type"]
    );
    assert_eq!(
        toon_list_fields(Some("id, status , req_type")).unwrap(),
        vec!["id", "status", "type"]
    );
    // Empty / whitespace-only selection falls back to the default.
    assert_eq!(
        toon_list_fields(Some("  ")).unwrap(),
        vec!["id", "title", "status", "type"]
    );
    assert!(toon_list_fields(Some("id,bogus")).is_err());
}

// TASK-964: cache status spellings collapse to a stable lowercase-hyphen
// token regardless of how the cache stored them. trace:TASK-964
#[test]
fn toon_status_token_normalizes() {
    assert_eq!(toon_status_token("InProgress"), "in-progress");
    assert_eq!(toon_status_token("in-progress"), "in-progress");
    assert_eq!(toon_status_token("NeedsAttention"), "needs-attention");
    assert_eq!(toon_status_token("Draft"), "draft");
    assert_eq!(toon_status_token("Done"), "done");
}

// STORY-734 test fixtures: a minimal RequirementSummary builder so the
// `--fields` projection + human table layout are unit-testable.
// trace:STORY-734 | ai:claude
fn fields_summary(
    id: &str,
    title: &str,
    status: &str,
    priority: &str,
) -> aida_core::RequirementSummary {
    aida_core::RequirementSummary {
        id: uuid::Uuid::new_v4(),
        spec_id: Some(id.to_string()),
        agreed_id: None,
        title: title.to_string(),
        description: String::new(),
        status: status.to_string(),
        priority: priority.to_string(),
        owner: String::new(),
        assignee: None,
        feature: String::new(),
        req_type: "story".to_string(),
        tags: Vec::new(),
        created_at: String::new(),
        modified_at: String::new(),
        archived: false,
        archived_at: None,
        deferred: false,
        deferred_at: None,
        deferred_until: None,
        in_degree: 0,
        out_degree: 0,
        heft: 0,
        blocked: false,
        // trace:TASK-1065 | ai:claude
        has_pending_decision: false,
        execution_mode: None,
        yaml_path: String::new(),
    }
}

// STORY-734: `--fields id,status,title` renders exactly those three columns,
// in that order, on the HUMAN table — header order + per-row cell order both
// follow the requested CSV, not the default id/type/status/priority layout.
// trace:STORY-734 | ai:claude
#[test]
fn fields_selects_and_orders_columns() {
    colored::control::set_override(false); // deterministic, no ANSI
    let selected = toon_list_fields(Some("id,status,title")).unwrap();
    assert_eq!(selected, vec!["id", "status", "title"]);

    let reqs = vec![fields_summary(
        "STORY-1",
        "first thing",
        "in-progress",
        "high",
    )];
    let lines = build_list_fields_lines(&reqs, &selected, |_| (false, false, false));

    // Header columns are exactly ID / Status / Title, in order.
    assert_eq!(
        lines[0].split_whitespace().collect::<Vec<_>>(),
        vec!["ID", "Status", "Title"]
    );
    // The data row leads with the id, then the normalized status token, then
    // the title — the chosen order, nothing else.
    let row = &lines[2];
    assert!(row.starts_with("STORY-1"));
    let id_at = row.find("STORY-1").unwrap();
    let status_at = row.find("in-progress").unwrap();
    let title_at = row.find("first thing").unwrap();
    assert!(id_at < status_at && status_at < title_at);
    // No stray default column (e.g. priority/type) leaked in.
    assert!(!row.contains("high"));
    assert!(!row.contains("story"));
}

// STORY-734: the agent/TOON projection emits ONLY the chosen fields — a
// 2-field selection is narrower than the 4-field default, so it's a real
// token win. The same `toon_list_fields` + `toon_list_cell` pair drives both
// the agent rows and the human table. trace:STORY-734 | ai:claude
#[test]
fn fields_agent_toon_emits_only_chosen_fields() {
    let selected = toon_list_fields(Some("id,priority")).unwrap();
    assert_eq!(selected, vec!["id", "priority"]);
    // Narrower than the default schema.
    assert!(selected.len() < toon_list_fields(None).unwrap().len());

    let req = fields_summary("STORY-9", "t", "approved", "low");
    let cells: Vec<String> = selected
        .iter()
        .map(|f| toon_list_cell(&req, (false, false, false), f))
        .collect();
    // Exactly two cells, in order, and nothing from the dropped columns.
    assert_eq!(cells, vec!["STORY-9".to_string(), "low".to_string()]);
}

// STORY-734: an unknown field name is rejected (not silently ignored — the
// bug being fixed) with the valid set named in the error. trace:STORY-734
#[test]
fn fields_unknown_name_errors_with_valid_set() {
    let err = toon_list_fields(Some("id,bogus")).unwrap_err().to_string();
    assert!(err.contains("bogus"), "names the offending field: {err}");
    assert!(err.contains("valid fields"), "names the valid set: {err}");
    // A few representative valid names appear in the guidance.
    assert!(err.contains("id"));
    assert!(err.contains("title"));
    assert!(err.contains("status"));
}

// STORY-734: `aida search --fields` is now humans+agents parity — agent mode
// emits the lean TOON `specs[N]{...}` projection (the ~2x token win), a human
// TTY emits the aligned box-table, and both surfaces resolve cells through the
// same `toon_list_fields`/`toon_list_cell` pair `aida list --fields` uses.
// Mirrors `fields_agent_toon_emits_only_chosen_fields` for the search handler.
// trace:STORY-734 trace:BUG-668 | ai:claude
#[test]
fn search_fields_agent_emits_toon_human_emits_table() {
    colored::control::set_override(false); // deterministic, no ANSI
    let selected = toon_list_fields(Some("id,status")).unwrap();
    assert_eq!(selected, vec!["id", "status"]);

    let results = vec![fields_summary(
        "STORY-1",
        "first thing",
        "in-progress",
        "high",
    )];

    // AGENT mode: the lean TOON projection — a `specs[N]{id,status}:` header
    // (NOT the human box-table) with the chosen columns only.
    let agent = render_search_fields(&results, &selected, true);
    assert!(
        agent.contains("specs[1]{id,status}:"),
        "agent mode emits the TOON specs table: {agent}"
    );
    assert!(agent.contains("STORY-1"));
    assert!(agent.contains("in-progress"));
    // The verbose human header label must NOT appear in the TOON stream.
    assert!(
        !agent.contains("Status"),
        "no capitalized box-table header leaks into agent output: {agent}"
    );

    // HUMAN mode: the aligned box-table — capitalized `ID`/`Status` headers,
    // a divider rule, and the row; NOT the TOON `specs[...]` shape.
    let human = render_search_fields(&results, &selected, false);
    assert!(
        !human.contains("specs["),
        "human mode emits the box-table, not TOON: {human}"
    );
    assert!(human.contains("ID") && human.contains("Status"));
    assert!(human.contains("─"), "box-table has a divider rule: {human}");
    assert!(human.contains("STORY-1") && human.contains("in-progress"));
    assert!(human.contains("1 results"));
}

// BUG-672: the DEFAULT `aida search` path (no `--fields`) now routes through
// the same lean TOON projection in agent mode instead of the hardcoded human
// box-table. The default schema is `toon_list_fields(None)` and the renderer
// is the shared `render_search_fields`, so agent mode emits `specs[N]{...}`
// with NO `─` rule, while the human path keeps the box-table (with the rule).
// trace:BUG-672
#[test]
fn search_default_agent_emits_toon_not_boxtable() {
    colored::control::set_override(false); // deterministic, no ANSI
    let selected = toon_list_fields(None).unwrap(); // the no-`--fields` schema
    assert_eq!(selected, vec!["id", "title", "status", "type"]);

    let results = vec![fields_summary(
        "STORY-1",
        "first thing",
        "in-progress",
        "high",
    )];

    // AGENT mode (what the default path passes): TOON `specs[...]` shape, the
    // full default schema, and crucially NO box-table divider rule.
    let agent = render_search_fields(&results, &selected, true);
    assert!(
        agent.contains("specs[1]{id,title,status,type}:"),
        "default agent search emits the TOON specs table: {agent}"
    );
    assert!(
        !agent.contains('─'),
        "no human box-table rule leaks into the default agent search: {agent}"
    );
    assert!(agent.contains("STORY-1") && agent.contains("in-progress"));

    // HUMAN mode keeps the aligned box-table with its divider rule.
    let human = render_search_fields(&results, &selected, false);
    assert!(
        !human.contains("specs["),
        "human default search keeps the box-table, not TOON: {human}"
    );
    assert!(
        human.contains('─'),
        "human box-table has a divider rule: {human}"
    );
}

// BUG-672 (Finding #4): both `aida search` and `aida graph` emit a trailing
// `next[]{cmd,to}` drill-in block — `aida show <id>` is the move an agent
// reaching for a result/neighbor is missing. trace:BUG-672
#[test]
fn search_and_graph_emit_next_block() {
    // Search with results suggests the placeholder drill-in.
    let s = crate::help_next::render(&crate::help_next::search_next(true)).unwrap();
    assert!(
        s.contains("next[1]{cmd,to}:"),
        "search emits a next block: {s}"
    );
    assert!(s.contains("aida show <id>"));
    // No results => no block.
    assert!(crate::help_next::search_next(false).is_empty());

    // Graph with neighbors suggests the placeholder drill-in; an empty
    // direction points at the queried spec's own detail.
    let g = crate::help_next::render(&crate::help_next::graph_next("BUG-672", true)).unwrap();
    assert!(
        g.contains("aida show <id>"),
        "graph emits a next block: {g}"
    );
    let g_empty =
        crate::help_next::render(&crate::help_next::graph_next("BUG-672", false)).unwrap();
    assert!(
        g_empty.contains("aida show BUG-672"),
        "empty-direction graph nudges the root spec: {g_empty}"
    );
}

// STORY-734: an unknown `--fields` name on `aida search` is rejected the same
// way `aida list` rejects it — the validation lives in the shared
// `toon_list_fields`, so the search surface inherits it. trace:STORY-734
#[test]
fn search_fields_unknown_name_errors() {
    let err = toon_list_fields(Some("id,bogus")).unwrap_err().to_string();
    assert!(err.contains("bogus"));
    assert!(err.contains("valid fields"));
}

// STORY-734: the default (no `--fields`) human path is unchanged — the
// dynamic renderer is invoked ONLY when `--fields` is present. The handler
// guards the dynamic table behind `fields.is_some()`, so `None` keeps the
// standard fixed-layout columns. trace:STORY-734 | ai:claude
#[test]
fn fields_default_none_keeps_standard_columns() {
    // The agent default schema is the historical id/title/status/type — a
    // None selection never collapses to the human `--fields` path.
    assert_eq!(
        toon_list_fields(None).unwrap(),
        vec!["id", "title", "status", "type"]
    );
}

// AGENT MODE = (AIDA_AGENT_OUTPUT truthy) OR (stdout not a TTY). An explicit
// falsey value force-selects the HUMAN path even when piped. trace:TASK-970
#[test]
fn agent_output_mode_env_truthy_wins_over_tty() {
    // Even at a TTY, an explicit truthy value selects agent mode.
    assert!(agent_output_mode_from(Some("1"), true));
    assert!(agent_output_mode_from(Some("true"), true));
    assert!(agent_output_mode_from(Some("yes"), true));
    assert!(agent_output_mode_from(Some("on"), true));
    // Arbitrary non-empty non-falsey value is also truthy.
    assert!(agent_output_mode_from(Some("agent"), true));
}

#[test]
fn agent_output_mode_env_falsey_forces_human_even_when_piped() {
    for v in ["0", "false", "no", "off", "", "  "] {
        assert!(
            !agent_output_mode_from(Some(v), false),
            "AIDA_AGENT_OUTPUT={v:?} should force the human path"
        );
    }
}

#[test]
fn agent_output_mode_unset_follows_tty() {
    // Unset: piped (no TTY) => agent mode; a real TTY => human path.
    assert!(agent_output_mode_from(None, false));
    assert!(!agent_output_mode_from(None, true));
}

// STORY-764: an explicit `--format` / AIDA_OUTPUT_FORMAT pin overrides the
// TTY-based default so piped scripts get a stable format (BUG-707).
#[test]
fn format_pin_human_forces_human_even_when_piped() {
    // stdout not a TTY (piped) AND AIDA_AGENT_OUTPUT truthy would BOTH
    // otherwise select agent mode — the `human` pin still wins.
    assert!(!resolve_agent_mode(Some(OutputFormat::Human), None, false));
    assert!(!resolve_agent_mode(
        Some(OutputFormat::Human),
        Some("1"),
        false
    ));
}

#[test]
fn format_pin_toon_and_json_force_agent_even_at_a_tty() {
    // A real TTY (and even an explicit falsey AIDA_AGENT_OUTPUT) would
    // otherwise select the human path — `toon`/`json` still force agent mode.
    assert!(resolve_agent_mode(Some(OutputFormat::Toon), None, true));
    assert!(resolve_agent_mode(Some(OutputFormat::Json), None, true));
    assert!(resolve_agent_mode(
        Some(OutputFormat::Toon),
        Some("0"),
        true
    ));
}

#[test]
fn format_pin_none_preserves_tty_based_behavior() {
    // No pin => byte-identical to the historical env/TTY resolution.
    assert!(resolve_agent_mode(None, None, false)); // piped => agent
    assert!(!resolve_agent_mode(None, None, true)); // TTY => human
    assert!(resolve_agent_mode(None, Some("1"), true)); // explicit env truthy
    assert!(!resolve_agent_mode(None, Some("0"), false)); // explicit env falsey
}

#[test]
fn output_format_env_parses_case_insensitively_with_aliases() {
    assert_eq!(
        parse_output_format(Some("human")),
        Some(OutputFormat::Human)
    );
    assert_eq!(
        parse_output_format(Some(" TABLE ")),
        Some(OutputFormat::Human)
    );
    assert_eq!(parse_output_format(Some("TOON")), Some(OutputFormat::Toon));
    assert_eq!(parse_output_format(Some("agent")), Some(OutputFormat::Toon));
    assert_eq!(parse_output_format(Some("Json")), Some(OutputFormat::Json));
    // Unset or unrecognized => None (defer to the TTY default, never error).
    assert_eq!(parse_output_format(None), None);
    assert_eq!(parse_output_format(Some("xml")), None);
    assert_eq!(parse_output_format(Some("")), None);
}

// BUG-671: a `Type 'y'` prompt is "non-interactive" — auto-confirm-or-fail,
// never silently cancel — when agent mode is forced OR stdin is not a TTY.
#[test]
fn non_interactive_confirm_when_no_human_at_keyboard() {
    // Agent mode forced (regardless of stdin) => non-interactive.
    assert!(non_interactive_confirm_from(true, true));
    assert!(non_interactive_confirm_from(true, false));
    // Not agent mode, but stdin is piped/closed (no TTY) => non-interactive:
    // reading a prompt would only hit EOF.
    assert!(non_interactive_confirm_from(false, false));
    // Genuine interactive shell: not agent mode AND stdin is a TTY.
    assert!(!non_interactive_confirm_from(false, true));
}

// BUG-721: the interactive `aida review <spec>` verb may launch its `claude
// -p` reviewer subprocess ONLY when a human is at the terminal (both stdin
// and stdout are TTYs). A non-interactive invocation (piped/redirected/CI/
// agent) must NOT silently spawn a blind headless reviewer — the gate below
// returns false so the verb refuses instead.
#[test]
fn review_reviewer_launch_requires_a_human_terminal() {
    // Human at a real terminal: both TTYs => may launch the reviewer.
    assert!(review_may_launch_reviewer(true, true));

    // Non-interactive stdin (piped/redirected/CI/agent) => do NOT launch.
    // This is the BUG-721 fix: `aida review` no longer spawns a headless
    // reviewer the caller can neither see nor interrupt.
    assert!(!review_may_launch_reviewer(false, true));
    // stdout redirected (captured) with a TTY stdin is likewise not the
    // human-watching-a-reviewer context.
    assert!(!review_may_launch_reviewer(true, false));
    // Fully non-interactive (the CI / agent case) => do NOT launch.
    assert!(!review_may_launch_reviewer(false, false));

    // NOTE: this gate is scoped to the INTERACTIVE verb `handle_review_spec`.
    // The orchestrator's drain review phase (`run_reviewer`) does not call
    // this helper — it shells out to `aida queue work PR-N` with
    // `AIDA_AUTO_COMPLETE=1` and launches its headless reviewer there on
    // purpose — so gating here leaves the autonomous drain untouched.
}

// BUG-671: agent mode shows ONLY the first line of an error as the TOON
// `error:` summary. The gated-write guard errors therefore have to name the
// override flag on that first line, or an agent never sees the escape hatch.
#[test]
fn reopen_guard_error_names_force_on_first_line() {
    // Mirror of the `aida edit <closed> --status approved` guard message.
    let msg = format!(
        "{} is currently {}. Re-opening a closed requirement is \
             usually a mistake — pass --force to override.\n  Otherwise, \
             file a new requirement that supersedes {}.",
        "BUG-1", "Completed", "BUG-1"
    );
    let (summary, _help) = agent_error_summary_help(&msg);
    assert!(
        summary.contains("--force"),
        "the agent-visible summary must name --force, got: {summary:?}"
    );
}

// TASK-972 (AXI #6): the agent-mode error block reuses the rich next-command
// hint AIDA's not-found errors already embed, and the summary drops any
// human `Error:` prefix so it doesn't double under the TOON `error:` key.
// BUG-684: a truly-empty listing (fresh repo, nothing hidden) yields the
// create signpost; a listing that's empty only because rows are filtered out
// (closed/archived/deferred hidden) yields None so the hidden-hints stand
// alone.
#[test]
fn empty_list_hint_only_fires_when_nothing_hidden() {
    let line = empty_list_hint_line(0, 0, 0).expect("fresh repo → create hint");
    assert!(line.contains("aida add --title"));
    assert!(line.contains("Nothing here yet"));

    // Any hidden bucket suppresses the create hint.
    assert!(empty_list_hint_line(3, 0, 0).is_none());
    assert!(empty_list_hint_line(0, 5, 0).is_none());
    assert!(empty_list_hint_line(0, 0, 2).is_none());
}

#[test]
fn agent_error_summary_strips_prefix_and_extracts_suggestion() {
    let msg = "Requirement not found: NOSUCH-1\n  \
                   Hint: check the spec ID (try `aida list` or `aida search <terms>`).";
    let (summary, help) = agent_error_summary_help(msg);
    assert_eq!(summary, "Requirement not found: NOSUCH-1");
    assert_eq!(help.as_deref(), Some("aida list"));
}

#[test]
fn agent_error_summary_drops_leading_error_prefix() {
    let (summary, help) = agent_error_summary_help("Error: brief not found at \"x\".");
    assert_eq!(summary, "brief not found at \"x\".");
    assert_eq!(help, None);
}

#[test]
fn agent_error_rendered_block_is_toon_on_stdout_shape() {
    // The exact block the agent-mode handler prints to STDOUT for the
    // acceptance case (`aida show NOSUCH-1`).
    let msg = "Requirement not found: NOSUCH-1\n  \
                   Hint: check the spec ID (try `aida list` or `aida search <terms>`).";
    let (summary, help) = agent_error_summary_help(msg);
    let block = toon::error_block(summary, help.as_deref());
    assert_eq!(
        block,
        "error: \"Requirement not found: NOSUCH-1\"\nhelp: aida list"
    );
}

#[test]
fn extract_aida_suggestion_finds_first_command_or_none() {
    assert_eq!(
        extract_aida_suggestion("try `aida list` or `aida search x`").as_deref(),
        Some("aida list")
    );
    // A backtick token that isn't an `aida …` command is skipped.
    assert_eq!(
        extract_aida_suggestion("see `requirements.db`, then `aida cache rebuild`").as_deref(),
        Some("aida cache rebuild")
    );
    assert_eq!(extract_aida_suggestion("plain message, no hint"), None);
    // An unterminated backtick doesn't panic or loop.
    assert_eq!(extract_aida_suggestion("dangling `aida list"), None);
}

// The default `aida list` cap applies ONLY to the default table render in
// agent mode: explicit `--limit`/`--all`, the `--short`/`--json` machine
// shapes, the `--tree` view, and the human TTY path all bypass it.
// trace:TASK-970
#[test]
fn list_default_cap_applies_to_bare_agent_table() {
    assert_eq!(
        agent_list_default_cap(None, false, false, false, false, true),
        Some(AGENT_LIST_DEFAULT_LIMIT)
    );
}

#[test]
fn list_default_cap_off_for_human_tty() {
    // agent_mode = false → never cap (human path is byte-identical).
    assert_eq!(
        agent_list_default_cap(None, false, false, false, false, false),
        None
    );
}

#[test]
fn list_default_cap_explicit_limit_and_all_override() {
    // An explicit --limit wins (the caller's value is applied elsewhere).
    assert_eq!(
        agent_list_default_cap(Some(5), false, false, false, false, true),
        None
    );
    // --all opts out of any cap.
    assert_eq!(
        agent_list_default_cap(None, true, false, false, false, true),
        None
    );
}

#[test]
fn list_default_cap_skips_machine_and_tree_shapes() {
    // --short and --json stay unbounded so existing enumerating consumers
    // keep working; --tree is the grouped view, also unbounded.
    assert_eq!(
        agent_list_default_cap(None, false, true, false, false, true),
        None
    );
    assert_eq!(
        agent_list_default_cap(None, false, false, true, false, true),
        None
    );
    assert_eq!(
        agent_list_default_cap(None, false, false, false, true, true),
        None
    );
}
