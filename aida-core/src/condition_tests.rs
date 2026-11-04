//! Unit tests for typed conditions: parsing, canonical round-trip and
//! evaluation against fake facts.
// trace:STORY-2402 | ai:claude

use super::*;
use std::collections::HashMap;

fn parse(s: &str) -> Condition {
    s.parse::<Condition>()
        .unwrap_or_else(|e| panic!("`{s}` should parse: {e}"))
}

fn parse_err(s: &str) -> String {
    match s.parse::<Condition>() {
        Ok(c) => panic!("`{s}` should not parse, got {c:?}"),
        Err(e) => e.to_string(),
    }
}

#[test]
fn every_form_parses_to_its_variant() {
    assert_eq!(
        parse("spec:STORY-12=completed"),
        Condition::Spec {
            id: "STORY-12".into(),
            status: RequirementStatus::Completed
        }
    );
    assert_eq!(
        parse("pr:2577=merged"),
        Condition::PrMerged { number: 2577 }
    );
    assert_eq!(
        parse("release:>=0.17"),
        Condition::Release {
            op: CmpOp::Ge,
            version: "0.17".parse().unwrap()
        }
    );
    assert_eq!(
        parse("date:2026-12-01"),
        Condition::Date {
            on: NaiveDate::from_ymd_opt(2026, 12, 1).unwrap()
        }
    );
    assert_eq!(
        parse("query:tag=operator-held,status=approved count>=5"),
        Condition::Query {
            filters: vec![
                QueryFilter::Tag("operator-held".into()),
                QueryFilter::Status(RequirementStatus::Approved)
            ],
            op: CmpOp::Ge,
            n: 5
        }
    );
    assert_eq!(parse("manual:joe"), Condition::Manual { who: "joe".into() });
}

#[test]
fn canonical_forms_round_trip() {
    for canonical in [
        "spec:STORY-12=completed",
        "spec:BUG-3=in-progress",
        "spec:TASK-9=needs-attention",
        "pr:2577=merged",
        "release:>=0.17",
        "release:>0.17.2",
        "release:<1",
        "release:<=2.0",
        "release:=0.16.0",
        "date:2026-12-01",
        "query:tag=operator-held,status=approved count>=5",
        "query:type=bug,feature=CLI count<3",
        "query:count=0",
        "manual:joe",
    ] {
        let cond = parse(canonical);
        assert_eq!(cond.to_string(), canonical, "display of {cond:?}");
        assert_eq!(parse(&cond.to_string()), cond, "re-parse of {canonical}");
    }
}

#[test]
fn lenient_spellings_normalise_to_the_canonical_form() {
    for (input, canonical) in [
        ("  SPEC:STORY-12 = Completed ", "spec:STORY-12=completed"),
        ("spec:STORY-12=In Progress", "spec:STORY-12=in-progress"),
        ("pr:#2577=MERGED", "pr:2577=merged"),
        ("release:0.17", "release:>=0.17"),
        ("release:v0.17.1", "release:>=0.17.1"),
        ("release:==1.2", "release:=1.2"),
        (
            "query:status=In-Progress count==2",
            "query:status=in-progress count=2",
        ),
    ] {
        assert_eq!(parse(input).to_string(), canonical, "input {input:?}");
    }
}

#[test]
fn malformed_conditions_are_refused_with_the_accepted_forms() {
    for (input, needle) in [
        ("when STORY-12 ships", "not a typed condition"),
        ("someday:soon", "unknown condition kind `someday`"),
        ("spec:STORY-12", "spec:<ID>=<status>"),
        ("spec:STORY-12=shipped", "`shipped` is not a spec status"),
        ("spec:=completed", "missing spec ID"),
        ("spec:STORY 12=completed", "must not contain spaces"),
        ("pr:abc=merged", "not a pull request number"),
        ("pr:12=closed", "pr state `closed` is not supported"),
        ("pr:12", "pr:<N>=merged"),
        ("release:>=", "is not a version"),
        ("release:>=0.17-rc1", "not a version like"),
        ("release:>=0..1", "not a version like"),
        ("date:next tuesday", "not a date"),
        ("date:2026-13-01", "not a date"),
        ("query:tag=x", "must be count<op><n>"),
        ("query:tag=x count", "needs a comparison"),
        ("query:tag=x count>=many", "not a whole number"),
        (
            "query:owner=joe count>=1",
            "unknown query filter key `owner`",
        ),
        (
            "query:status=later count>=1",
            "`later` is not a spec status",
        ),
        ("query:tag= count>=1", "has no value"),
        ("query:tag count>=1", "must be <key>=<value>"),
        (
            "query:a=b c=d count>=1",
            "query:<key>=<value>[,...] count<op><n>",
        ),
        ("manual:", "missing manual owner"),
        ("manual:joe and kim", "must not contain spaces"),
    ] {
        let message = parse_err(input);
        assert!(
            message.contains(needle),
            "input {input:?}: `{message}` lacks `{needle}`"
        );
        assert!(
            message.contains(CONDITION_FORMS),
            "input {input:?}: `{message}` lacks the forms list"
        );
    }
}

#[test]
fn versions_compare_with_zero_padding_and_latest_skips_non_releases() {
    let v = |s: &str| s.parse::<Version>().unwrap();
    assert_eq!(
        v("0.17").cmp_padded(&v("0.17.0")),
        std::cmp::Ordering::Equal
    );
    assert_eq!(
        v("0.17.1").cmp_padded(&v("0.17")),
        std::cmp::Ordering::Greater
    );
    assert_eq!(v("0.9").cmp_padded(&v("0.17")), std::cmp::Ordering::Less);
    assert_eq!(
        Version::latest_of(["v0.16.0", "v0.17.0-rc1", "v0.9.9", "nightly", "0.16.2"]),
        Some(v("0.16.2"))
    );
    assert_eq!(Version::latest_of(["nightly"]), None);
}

struct FakeFacts {
    specs: HashMap<&'static str, RequirementStatus>,
    store_broken: bool,
    prs: HashMap<u64, bool>,
    release: Result<Option<Version>, String>,
    today: NaiveDate,
    reqs: Vec<Requirement>,
}

impl Default for FakeFacts {
    fn default() -> Self {
        Self {
            specs: HashMap::new(),
            store_broken: false,
            prs: HashMap::new(),
            release: Ok(None),
            today: NaiveDate::from_ymd_opt(2026, 11, 4).unwrap(),
            reqs: Vec::new(),
        }
    }
}

impl ConditionFacts for FakeFacts {
    fn spec_status(&self, id: &str) -> Result<Option<RequirementStatus>, String> {
        if self.store_broken {
            return Err("store unreadable".into());
        }
        Ok(self.specs.get(id).cloned())
    }
    fn pr_merged(&self, number: u64) -> Option<bool> {
        self.prs.get(&number).copied()
    }
    fn latest_release(&self) -> Result<Option<Version>, String> {
        self.release.clone()
    }
    fn today(&self) -> NaiveDate {
        self.today
    }
    fn count(&self, filters: &[QueryFilter]) -> Result<u64, String> {
        if self.store_broken {
            return Err("store unreadable".into());
        }
        Ok(count_matching(filters, &self.reqs))
    }
}

fn verdict(cond: &str, facts: &FakeFacts) -> Verdict {
    parse(cond)
        .evaluate(facts)
        .unwrap_or_else(|e| panic!("`{cond}` should evaluate: {e}"))
        .verdict
}

#[test]
fn spec_condition_compares_status_and_errors_on_a_missing_spec() {
    let mut facts = FakeFacts::default();
    facts.specs.insert("STORY-12", RequirementStatus::Completed);
    facts
        .specs
        .insert("STORY-13", RequirementStatus::InProgress);
    assert_eq!(verdict("spec:STORY-12=completed", &facts), Verdict::Met);
    let eval = parse("spec:STORY-13=completed").evaluate(&facts).unwrap();
    assert_eq!(eval.verdict, Verdict::NotMet);
    assert_eq!(eval.observed, "STORY-13 is in-progress");

    let missing = parse("spec:STORY-99=completed")
        .evaluate(&facts)
        .unwrap_err();
    assert!(
        missing.0.contains("no spec has the ID STORY-99"),
        "{missing}"
    );

    facts.store_broken = true;
    let broken = parse("spec:STORY-12=completed")
        .evaluate(&facts)
        .unwrap_err();
    assert!(
        broken.0.contains("could not read the spec store"),
        "{broken}"
    );
}

#[test]
fn pr_condition_is_unknown_not_unmet_when_the_state_cannot_be_read() {
    let mut facts = FakeFacts::default();
    facts.prs.insert(1, true);
    facts.prs.insert(2, false);
    assert_eq!(verdict("pr:1=merged", &facts), Verdict::Met);
    assert_eq!(verdict("pr:2=merged", &facts), Verdict::NotMet);
    assert_eq!(verdict("pr:3=merged", &facts), Verdict::Unknown);
}

#[test]
fn release_condition_compares_the_latest_release() {
    let mut facts = FakeFacts {
        release: Ok(Some("0.17.1".parse().unwrap())),
        ..FakeFacts::default()
    };
    assert_eq!(verdict("release:>=0.17", &facts), Verdict::Met);
    assert_eq!(verdict("release:>0.17.1", &facts), Verdict::NotMet);
    assert_eq!(verdict("release:<0.18", &facts), Verdict::Met);
    assert_eq!(verdict("release:=0.17.1", &facts), Verdict::Met);
    assert_eq!(verdict("release:<=0.17", &facts), Verdict::NotMet);

    facts.release = Ok(None);
    assert_eq!(verdict("release:>=0.1", &facts), Verdict::NotMet);
    facts.release = Err("git failed".into());
    assert_eq!(verdict("release:>=0.1", &facts), Verdict::Unknown);
}

#[test]
fn date_condition_is_met_on_and_after_the_day() {
    let facts = FakeFacts::default(); // today is 2026-11-04
    assert_eq!(verdict("date:2026-11-03", &facts), Verdict::Met);
    assert_eq!(verdict("date:2026-11-04", &facts), Verdict::Met);
    assert_eq!(verdict("date:2026-11-05", &facts), Verdict::NotMet);
}

fn req(
    status: RequirementStatus,
    ty: crate::models::RequirementType,
    tags: &[&str],
) -> Requirement {
    let mut r = Requirement::new("t".into(), "d".into());
    r.status = status;
    r.req_type = ty;
    r.feature = "CLI".into();
    r.tags = tags.iter().map(|t| t.to_string()).collect();
    r
}

#[test]
fn query_condition_counts_non_archived_specs_passing_every_filter() {
    use crate::models::RequirementType as T;
    let mut archived = req(RequirementStatus::Approved, T::Story, &["operator-held"]);
    archived.archived = true;
    let facts = FakeFacts {
        reqs: vec![
            req(RequirementStatus::Approved, T::Story, &["operator-held"]),
            req(RequirementStatus::Approved, T::Bug, &["Operator-Held"]),
            req(RequirementStatus::Draft, T::Story, &["operator-held"]),
            req(RequirementStatus::Approved, T::Story, &[]),
            archived,
        ],
        ..FakeFacts::default()
    };
    assert_eq!(
        verdict("query:tag=operator-held,status=approved count=2", &facts),
        Verdict::Met
    );
    assert_eq!(
        verdict("query:tag=operator-held,status=approved count>2", &facts),
        Verdict::NotMet
    );
    assert_eq!(verdict("query:type=story count=3", &facts), Verdict::Met);
    assert_eq!(verdict("query:type=BUG count=1", &facts), Verdict::Met);
    assert_eq!(verdict("query:feature=cli count=4", &facts), Verdict::Met);
    assert_eq!(verdict("query:count=4", &facts), Verdict::Met);
    assert_eq!(verdict("query:feature=TUI count<1", &facts), Verdict::Met);

    let broken = FakeFacts {
        store_broken: true,
        ..FakeFacts::default()
    };
    assert!(parse("query:count>=0").evaluate(&broken).is_err());
}

#[test]
fn manual_condition_never_evaluates_true() {
    let eval = parse("manual:joe").evaluate(&FakeFacts::default()).unwrap();
    assert_eq!(eval.verdict, Verdict::Manual);
    assert_eq!(eval.observed, "waiting on joe to decide");
    assert_eq!(Verdict::Manual.as_str(), "manual");
}
