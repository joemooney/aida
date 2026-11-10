//! Observational metrics only; no gate decisions or persisted report state.
// trace:STORY-2699 | ai:codex
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use clap::{CommandFactory, FromArgMatches};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
#[path = "flow_cmd_args.rs"]
mod args;
use args::*;

pub(crate) static FLOW: crate::cmdkit::CommandReg = crate::cmdkit::CommandReg {
    name: "flow",
    help_after: "findings",
    command: FlowArgs::command,
    policy: |_| crate::cmdkit::CmdPolicy {
        stakeholder: crate::cmdkit::StakeholderAction::Read,
        per_write_auto_push: false,
        skip_role_restore_prompt: false,
        team_identity_write: false,
    },
    run: crate::cmdkit::Handler::Early(run),
};

// Ordered, first-match classifier from the advisor's cr_reasons.py.
const CLASSIFIERS: &[(&str, &str)] = &[
    (
        "ci-failure",
        r"\bCI\b.*(fail|red)|fails? (in )?CI|check formatting|rustfmt|cargo fmt|clippy|job \d+|test-time|timing budget",
    ),
    (
        "metadata/title/trailer",
        r"\btitle\b|trailer|\(Refs|completion|PR body|change notes|subject",
    ),
    (
        "scope-incomplete",
        r"incomplete|not implemented|unimplemented|remain(s)? (unimplemented|open)|missing (the )?(required|acceptance)|acceptance.*(unmet|not met)|does not (implement|cover)",
    ),
    (
        "tests-missing",
        r"\btest(s)?\b.*(missing|absent|add|no regression)|regression (test|coverage)|coverage",
    ),
    (
        "conflict/merge",
        r"conflict|merge (origin|main)|behind main|stale base",
    ),
    (
        "correctness-defect",
        r"\bbug\b|incorrect|wrong|panic|race|leak|refus|bypass|unsafe|security|fail[- ]open|violat",
    ),
];
fn classify(text: &str) -> &'static str {
    static RULES: std::sync::OnceLock<Vec<(&str, regex::Regex)>> = std::sync::OnceLock::new();
    RULES
        .get_or_init(|| {
            CLASSIFIERS
                .iter()
                .map(|(k, r)| {
                    (
                        *k,
                        regex::RegexBuilder::new(r)
                            .case_insensitive(true)
                            .build()
                            .unwrap(),
                    )
                })
                .collect()
        })
        .iter()
        .find(|(_, r)| r.is_match(text))
        .map_or("other", |(k, _)| *k)
}
fn severity(text: &str) -> String {
    static RX: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RX.get_or_init(|| {
        regex::Regex::new(r"\b(P[0-3]|BLOCKER|BLOCKING|blocking|minor|nit|cosmetic)\b").unwrap()
    })
    .find(text)
    .map_or("UNTAGGED".into(), |m| m.as_str().to_uppercase())
}
fn timestamp(v: &Value) -> Option<DateTime<Utc>> {
    v.as_str()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.with_timezone(&Utc))
}
fn bound(s: &str) -> Result<DateTime<Utc>> {
    timestamp(&json!(s))
        .or_else(|| timestamp(&json!(format!("{s}T00:00:00Z"))))
        .context("expected RFC3339 timestamp or YYYY-MM-DD")
}
fn hours(a: DateTime<Utc>, b: DateTime<Utc>) -> Option<f64> {
    (b >= a).then(|| (b - a).num_milliseconds() as f64 / 3_600_000.0)
}
fn walk<'a>(v: &'a Value, verdicts: &mut Vec<&'a Value>) {
    match v {
        Value::Object(o) => {
            if o.get("kind").and_then(Value::as_str) == Some("verdict") && v["record"].is_object() {
                verdicts.push(v);
            }
            for child in o.values() {
                walk(child, verdicts);
            }
        }
        Value::Array(a) => {
            for child in a {
                walk(child, verdicts);
            }
        }
        _ => (),
    }
}
fn measure(
    pr: u64,
    title: &str,
    landed: DateTime<Utc>,
    meta: &Value,
    ledger: &[Value],
    runs: &[Value],
    ledger_present: bool,
) -> Result<Value> {
    let created = timestamp(&meta["createdAt"]).context("forge PR has no valid createdAt")?;
    let commits = meta["commits"]
        .as_array()
        .context("forge PR has no commits array")?;
    let mut verdicts = vec![];
    for event in ledger {
        walk(event, &mut verdicts);
    }
    let mut counts = BTreeMap::<String, usize>::new();
    let mut sequence = vec![];
    let mut findings = vec![];
    let mut seen = BTreeSet::new();
    for event in verdicts {
        let r = &event["record"];
        let kind = r["verdict"]
            .as_str()
            .or(r["kind"].as_str())
            .or(r["decision"].as_str())
            .unwrap_or("?")
            .to_uppercase();
        let at = timestamp(&r["recorded_at"]).or_else(|| timestamp(&event["at"]));
        if let Some(at) = at.filter(|at| *at <= landed) {
            *counts.entry(kind.clone()).or_default() += 1;
            sequence.push((at, kind.clone()));
            if kind.contains("REQUEST") {
                for f in r["findings"].as_array().into_iter().flatten() {
                    let text = f
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| f.to_string());
                    if seen.insert(text.chars().take(120).collect::<String>()) {
                        findings.push(json!({"category":classify(&text),"severity":severity(&text),"text":text}));
                    }
                }
            }
        }
    }
    sequence.sort();
    let first = sequence.first().and_then(|(at, _)| hours(created, *at));
    // A later refusal invalidates an earlier approval for this timing metric.
    let approval = sequence
        .last()
        .filter(|(_, k)| matches!(k.as_str(), "APPROVED" | "APPROVE" | "PASS"))
        .and_then(|(at, _)| hours(*at, landed));
    let ci: Vec<Value> = runs.iter().map(|r| json!({"id":r["databaseId"], "attempt":r["attempt"], "name":r["workflowName"], "hours": timestamp(&r["startedAt"]).zip(timestamp(&r["updatedAt"])).filter(|_| r["status"] == "completed").and_then(|(a,b)| hours(a,b))})).collect();
    Ok(
        json!({"pr":pr,"title":title,"created_at":created,"landed_at":landed,
        "open_hours":hours(created,landed),"pushes":commits.len(),"pushes_basis":"commit-count proxy (advisor definition)",
        "ledger_present":ledger_present,"verdict_counts":counts,"first_review_hours":first,"approval_to_land_hours":approval,
        "ci_runs":ci,"train_hours":null,
        "findings":findings}),
    )
}
fn distribution(mut values: Vec<f64>) -> Value {
    values.sort_by(f64::total_cmp);
    if values.is_empty() {
        return json!({"count":0,"median":null,"p90":null});
    }
    let n = values.len();
    let median = if n % 2 == 0 {
        (values[n / 2 - 1] + values[n / 2]) / 2.0
    } else {
        values[n / 2]
    };
    json!({"count":n,"median":median,"p90":values[(n*9).div_ceil(10)-1]})
}
fn aggregate(rows: &[Value]) -> Value {
    let mut out = serde_json::Map::new();
    for key in [
        "open_hours",
        "pushes",
        "first_review_hours",
        "approval_to_land_hours",
        "train_hours",
    ] {
        out.insert(
            key.into(),
            distribution(rows.iter().filter_map(|r| r[key].as_f64()).collect()),
        );
    }
    out.insert(
        "ci_run_hours".into(),
        distribution(
            rows.iter()
                .flat_map(|r| r["ci_runs"].as_array().into_iter().flatten())
                .filter_map(|r| r["hours"].as_f64())
                .collect(),
        ),
    );
    let mut verdicts = BTreeMap::<String, u64>::new();
    let mut categories = BTreeMap::<String, usize>::new();
    let mut severities = BTreeMap::<String, usize>::new();
    for row in rows {
        if let Some(counts) = row["verdict_counts"].as_object() {
            for (kind, count) in counts {
                *verdicts.entry(kind.clone()).or_default() += count.as_u64().unwrap_or(0);
            }
        }
        for finding in row["findings"].as_array().into_iter().flatten() {
            *categories
                .entry(finding["category"].as_str().unwrap_or("other").into())
                .or_default() += 1;
            *severities
                .entry(finding["severity"].as_str().unwrap_or("UNTAGGED").into())
                .or_default() += 1;
        }
    }
    out.insert("verdict_totals".into(), json!(verdicts));
    out.insert("finding_categories".into(), json!(categories));
    out.insert("finding_severities".into(), json!(severities));
    Value::Object(out)
}
fn read_ledger(path: &Path) -> Result<(Vec<Value>, bool)> {
    let body = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((vec![], false)),
        Err(e) => return Err(e.into()),
    };
    Ok((
        body.lines()
            .filter(|s| !s.trim().is_empty())
            .enumerate()
            .map(|(i, s)| {
                serde_json::from_str(s)
                    .with_context(|| format!("{}:{}: malformed ledger", path.display(), i + 1))
            })
            .collect::<Result<_>>()?,
        true,
    ))
}
// Train coverage is stored on the train PR, with member PRs in proof.sources.
fn train_members(shared: &Path) -> Result<BTreeMap<u64, BTreeSet<u64>>> {
    fn visit(value: &Value, train: u64, out: &mut BTreeMap<u64, BTreeSet<u64>>) {
        if value["kind"] == "train-coverage" {
            for source in value["proof"]["sources"].as_array().into_iter().flatten() {
                if let Some(member) = source["pr"].as_u64() {
                    out.entry(member).or_default().insert(train);
                }
            }
        }
        match value {
            Value::Object(o) => {
                for child in o.values() {
                    visit(child, train, out);
                }
            }
            Value::Array(a) => {
                for child in a {
                    visit(child, train, out);
                }
            }
            _ => (),
        }
    }
    let dir = shared.join(".aida/gate-ledger");
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(e.into()),
    };
    let mut out = BTreeMap::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(train) = name
            .strip_prefix("PR-")
            .and_then(|s| s.strip_suffix(".jsonl"))
            .and_then(|s| s.parse().ok())
        else {
            continue;
        };
        let (events, _) = read_ledger(&entry.path())?;
        for event in events {
            visit(&event, train, &mut out);
        }
    }
    Ok(out)
}
fn collect(
    root: &Path,
    shared: &Path,
    log: &str,
    since: DateTime<Utc>,
    until: DateTime<Utc>,
    mut forge: impl FnMut(&Path, &[&str]) -> Result<Value>,
) -> Result<Vec<Value>> {
    let rx = regex::Regex::new(r"\(#(\d+)\)\s*$")?;
    let mut seen = BTreeSet::new();
    let mut rows = vec![];
    let trains = train_members(shared)?;
    for line in log.lines() {
        let Some((date, title)) = line.split_once('\t') else {
            continue;
        };
        let Some(c) = rx.captures(title) else {
            continue;
        };
        let landed = bound(date)?;
        if landed < since || landed > until {
            continue;
        }
        let pr: u64 = c[1].parse()?;
        if !seen.insert(pr) {
            continue;
        }
        let number = pr.to_string();
        let meta = forge(
            root,
            &["pr", "view", &number, "--json", "createdAt,commits"],
        )?;
        let commits = meta["commits"]
            .as_array()
            .context("forge PR missing commits")?;
        anyhow::ensure!(
            commits.len() < 100,
            "PR #{pr}: commit list may be truncated"
        );
        let mut runs = vec![];
        let mut run_ids = BTreeSet::new();
        for commit in commits {
            let sha = commit["oid"].as_str().context("forge commit missing oid")?;
            let result = forge(
                root,
                &[
                    "run",
                    "list",
                    "--commit",
                    sha,
                    "--limit",
                    "1000",
                    "--json",
                    "databaseId,attempt,workflowName,startedAt,updatedAt,status",
                ],
            )?;
            let array = result
                .as_array()
                .context("forge runs reply is not an array")?;
            anyhow::ensure!(array.len() < 1000, "PR #{pr}: run list may be truncated");
            for r in array {
                let id = r["databaseId"].as_u64().context("forge run missing id")?;
                if run_ids.insert(id) {
                    runs.push(r.clone());
                }
            }
        }
        let (ledger, present) = read_ledger(
            &shared
                .join(".aida/gate-ledger")
                .join(format!("PR-{pr}.jsonl")),
        )?;
        let mut row = measure(pr, title, landed, &meta, &ledger, &runs, present)?;
        let mut train_runs = vec![];
        for train in trains.get(&pr).into_iter().flatten() {
            let number = train.to_string();
            let data = forge(
                root,
                &["pr", "view", &number, "--json", "createdAt,mergedAt"],
            )?;
            train_runs.push(json!({"pr":train,"hours":timestamp(&data["createdAt"]).zip(timestamp(&data["mergedAt"])).and_then(|(a,b)| hours(a,b))}));
        }
        row["train_hours"] = train_runs
            .iter()
            .filter_map(|r| r["hours"].as_f64())
            .max_by(f64::total_cmp)
            .map_or(Value::Null, |n| json!(n));
        row["trains"] = json!(train_runs);
        rows.push(row);
    }
    Ok(rows)
}
fn run(matches: &clap::ArgMatches, _: &crate::cmdkit::DispatchCtx) -> Result<()> {
    let FlowCommand::Report { since, until, json } = FlowArgs::from_arg_matches(matches)?.command;
    let since = bound(&since)?;
    let until = until
        .as_deref()
        .map(bound)
        .transpose()?
        .unwrap_or_else(Utc::now);
    anyhow::ensure!(since <= until, "since must not be later than until");
    let root = crate::find_project_root()?;
    let shared = crate::gate::ledger::store_root(&root)?;
    let mut cmd = std::process::Command::new("git");
    cmd.current_dir(&root)
        .args(["log", "origin/main", "--format=%cI\t%s"]);
    let output = crate::command_output_with_timeout(cmd, std::time::Duration::from_secs(15))
        .context("git history timed out or failed to spawn")?;
    anyhow::ensure!(
        output.status.success(),
        "git merge history failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rows = collect(
        &root,
        &shared,
        &String::from_utf8(output.stdout)?,
        since,
        until,
        crate::forge::flow_report_read,
    )?;
    let report = json!({"since":since,"until":until,"prs":rows,"aggregate":aggregate(&rows),"notes":["Pushes are a commit-count proxy, not push events.","CI wall time uses startedAt to updatedAt for completed runs; queue time excluded, rerun history may be unavailable.","Train time is the longest merged train PR lifetime linked by local coverage evidence; absent evidence is unknown.","Missing ledger is unknown; verdict counts cover available local evidence only."]});
    if json || crate::output_format_is_json() {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("PR     open_h pushes first_review_h approval_land_h train_h verdicts / CI runs / findings");
        for r in &rows {
            println!(
                "{:<6} {:>6} {:>6} {:>14} {:>15} {:>7} {} / {} / {}",
                r["pr"],
                display(&r["open_hours"]),
                r["pushes"],
                display(&r["first_review_hours"]),
                display(&r["approval_to_land_hours"]),
                display(&r["train_hours"]),
                r["verdict_counts"],
                r["ci_runs"],
                r["findings"]
            );
        }
        println!("Aggregate (hours except pushes): {}", report["aggregate"]);
        for note in report["notes"].as_array().unwrap() {
            println!("{}", note.as_str().unwrap());
        }
    }
    Ok(())
}
fn display(v: &Value) -> String {
    v.as_f64().map_or("unknown".into(), |n| format!("{n:.2}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    // trace:STORY-2699 | ai:codex
    #[test]
    fn flow_json_projection_passes_dispatch_gate() {
        for spelling in ["--format=json", "--json"] {
            let mut argv: Vec<String> =
                ["aida", "flow", "report", "--since", "2026-11-09", spelling]
                    .into_iter()
                    .map(str::to_string)
                    .collect();
            crate::json_capability_gate(&mut argv).unwrap();
            let matches = crate::command_registry::built_command()
                .try_get_matches_from(argv)
                .unwrap();
            let flow = matches.subcommand_matches("flow").unwrap();
            assert!(matches!(
                FlowArgs::from_arg_matches(flow).unwrap().command,
                FlowCommand::Report { json: true, .. }
            ));
        }
    }
    #[test]
    fn flow_registered_read_only_args() {
        let cmd = crate::command_registry::built_command();
        let matches = cmd
            .try_get_matches_from([
                "aida",
                "flow",
                "report",
                "--since",
                "2026-11-09",
                "--format",
                "json",
            ])
            .unwrap();
        let sub = matches.subcommand_matches("flow").unwrap();
        assert_eq!(
            (FLOW.policy)(sub).stakeholder,
            crate::cmdkit::StakeholderAction::Read
        );
        assert!(matches!(
            FlowArgs::from_arg_matches(sub).unwrap().command,
            FlowCommand::Report { .. }
        ));
        assert_eq!(
            sub.get_one::<crate::cli::OutputFormat>("format"),
            Some(&crate::cli::OutputFormat::Json)
        );
    }
    #[test]
    fn flow_classifier_reference_table_and_priority() {
        for (text, cat) in [
            ("CI is red and title wrong", "ci-failure"),
            ("P1 title trailer", "metadata/title/trailer"),
            (
                "acceptance not met; regression coverage",
                "scope-incomplete",
            ),
            ("tests missing", "tests-missing"),
            ("stale base conflict", "conflict/merge"),
            ("unsafe race", "correctness-defect"),
            ("please explain", "other"),
        ] {
            assert_eq!(classify(text), cat);
        }
        assert_eq!(severity("P2 wrong result"), "P2");
        assert_eq!(severity("blocking regression"), "BLOCKING");
        assert_eq!(severity("nothing"), "UNTAGGED");
    }
    #[test]
    fn flow_fixture_ledger_fake_forge_all_metrics_and_no_writes() {
        let dir = tempfile::tempdir().unwrap();
        let ledger_dir = dir.path().join(".aida/gate-ledger");
        std::fs::create_dir_all(&ledger_dir).unwrap();
        let path = ledger_dir.join("PR-42.jsonl");
        let body=concat!(
            "{\"nested\":[{\"kind\":\"verdict\",\"record\":{\"verdict\":\"CHANGES REQUESTED\",\"recorded_at\":\"2026-11-09T01:00:00Z\",\"findings\":[\"P1 CI failed\",\"minor title\"]}}]}\n",
            "{\"kind\":\"train-coverage\",\"at\":\"2026-11-09T03:00:00Z\"}\n",
            "{\"kind\":\"verdict\",\"record\":{\"verdict\":\"APPROVED\",\"recorded_at\":\"2026-11-09T02:00:00Z\"}}\n");
        std::fs::write(&path, body).unwrap();
        std::fs::write(
            ledger_dir.join("PR-99.jsonl"),
            json!({"kind":"train-coverage","proof":{"sources":[{"pr":42}]}}).to_string(),
        )
        .unwrap();
        let since = bound("2026-11-09").unwrap();
        let until = bound("2026-11-10").unwrap();
        let rows=collect(dir.path(),dir.path(),"2026-11-09T04:00:00Z\tsubject (#42)\n2026-11-08T04:00:00Z\told (#1)",since,until,|_,argv| {
            match argv {
                ["pr","view","99",..]=>Ok(json!({"createdAt":"2026-11-09T03:00:00Z","mergedAt":"2026-11-09T04:00:00Z"})),
                ["pr","view","42",..]=>Ok(json!({"createdAt":"2026-11-09T00:00:00Z","commits":[{"oid":"a"},{"oid":"b"}]})),
                ["run","list",..]=>Ok(json!([{"databaseId":7,"attempt":1,"workflowName":"Build","status":"completed","startedAt":"2026-11-09T00:00:00Z","updatedAt":"2026-11-09T00:30:00Z"}])),
                _=>panic!("unexpected forge request {argv:?}")
            }
        }).unwrap();
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        for (key, value) in [
            ("open_hours", 4.0),
            ("first_review_hours", 1.0),
            ("approval_to_land_hours", 2.0),
            ("train_hours", 1.0),
        ] {
            assert_eq!(r[key], json!(value));
        }
        assert_eq!(r["pushes"], 2);
        assert_eq!(r["ci_runs"].as_array().unwrap().len(), 1);
        assert_eq!(r["ci_runs"][0]["hours"], 0.5);
        assert_eq!(r["verdict_counts"]["CHANGES REQUESTED"], 1);
        assert_eq!(r["findings"][0]["category"], "ci-failure");
        assert_eq!(r["findings"][0]["severity"], "P1");
        assert_eq!(aggregate(&rows)["open_hours"]["median"], 4.0);
        assert_eq!(std::fs::read_to_string(path).unwrap(), body);
        assert_eq!(std::fs::read_dir(ledger_dir).unwrap().count(), 2);
    }
    #[test]
    fn flow_refusal_dedup_and_unknown_ci() {
        let meta = json!({"createdAt":"2026-11-09T00:00:00Z", "commits":[]});
        let ledger = vec![
            json!({"kind":"verdict","record":{"verdict":"APPROVED","recorded_at":"2026-11-09T01:00:00Z"}}),
            json!({"kind":"verdict","record":{"verdict":"CHANGES REQUESTED","recorded_at":"2026-11-09T02:00:00Z","findings":["P2 wrong result","P2 wrong result"]}}),
            json!({"kind":"verdict","record":{"verdict":"APPROVED","recorded_at":"2026-11-11T01:00:00Z"}}),
        ];
        let runs =
            vec![json!({"databaseId":1,"status":"in_progress","startedAt":"2026-11-09T01:00:00Z"})];
        let row = measure(
            1,
            "x",
            bound("2026-11-10").unwrap(),
            &meta,
            &ledger,
            &runs,
            true,
        )
        .unwrap();
        assert_eq!(row["approval_to_land_hours"], Value::Null);
        assert_eq!(row["verdict_counts"]["APPROVED"], 1);
        assert_eq!(row["findings"].as_array().unwrap().len(), 1);
        assert_eq!(row["ci_runs"][0]["hours"], Value::Null);
        let summary = aggregate(&[row]);
        assert_eq!(summary["finding_categories"]["correctness-defect"], 1);
        assert_eq!(summary["finding_severities"]["P2"], 1);
        assert_eq!(summary["ci_run_hours"]["count"], 0);
    }
    #[test]
    fn flow_missing_bad_evidence_and_percentiles() {
        assert!(bound("bad").is_err());
        assert_eq!(distribution(vec![1., 2., 3., 4.])["median"], 2.5);
        assert_eq!(distribution((1..=10).map(f64::from).collect())["p90"], 9.0);
        assert_eq!(distribution(vec![])["median"], Value::Null);
        let r = measure(
            1,
            "x",
            bound("2026-11-10").unwrap(),
            &json!({"createdAt":"2026-11-09T00:00:00Z","commits":[]}),
            &[],
            &[],
            false,
        )
        .unwrap();
        assert_eq!(r["first_review_hours"], Value::Null);
        assert_eq!(r["train_hours"], Value::Null);
        assert_eq!(r["ledger_present"], false);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad");
        std::fs::write(&path, "{broken").unwrap();
        assert!(read_ledger(&path).is_err());
    }
}
