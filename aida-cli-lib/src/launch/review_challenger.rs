// trace:STORY-3120 | ai:claude
//! `aida review prepare --run-review --adversarial`: after each batch's
//! finder pass, a challenger must reproduce or refute every finding. The
//! challenger is a `RunnerCall` like the finder (same hardened spawn, deadline,
//! caps and trusted-base tuning) and, where one is installed, a vendor that
//! authored none of the change.

use super::review_runner::RunnerCall;
use super::review_tuning::{resolve_challenger, step_up_challenger_effort, TrustedBase};
use crate::vendors::review_runner::{
    review_vendor, review_vendor_ext, ReviewVendor, REVIEW_VENDORS,
};
use aida_core::review_engine::RawReviewFinding;
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

// trace:STORY-3120 | ai:antigravity
const CHALLENGE_HEAD: &str = "You are the challenger in a two-pass code review. \
For each numbered finding below, try to reproduce the defect from the change shown. \
Confirm (`reproduced`) ONLY if ALL THREE conditions hold: (1) Technically True in the actual code, \
(2) Introduced or Materially Exacerbated by This Change (not an untouched pre-existing issue), \
and (3) Non-Trivial & Actionable (not subjective style or compiler/linter redundancy). \
Refute (`refuted`) it when any condition fails. Reply with JSON only, one verdict per \
finding: {\"verdicts\":[{\"index\":<n>,\"verdict\":\"reproduced|refuted\",\
\"reason\":\"...\"}]}\n\n";

/// A finder finding the challenger refuted, with its reason.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RefutedFinding {
    pub finding: RawReviewFinding,
    pub reason: String,
}

/// What the adversarial pass did, for the bundle.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct AdversarialSummary {
    pub challenger_agent: Option<String>,
    pub challenger_effort: Option<String>,
    pub ai_authors: Vec<String>,
    pub refuted: Vec<RefutedFinding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Why the challenger could not run; set, the report is partial.
    // trace:STORY-3120 | ai:claude
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub challenger_unavailable: Option<String>,
    /// Finder findings the challenger never judged. They still gate.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unchallenged: Vec<RawReviewFinding>,
}

/// Review vendors named by `[AI:<tool>]` tags (`+` joins several) in commit
/// messages. The tags are the change's own claim: they choose only which
/// hardened vendor challenges, never whether a challenge runs.
pub(crate) fn detect_ai_authors(messages: &str) -> BTreeSet<&'static str> {
    let lower = messages.to_ascii_lowercase();
    let mut found = BTreeSet::new();
    for tag in lower.split("[ai:").skip(1) {
        let Some(end) = tag.find(']') else { continue };
        let tools = tag[..end].split(':').next().unwrap_or_default();
        for tool in tools.split('+').map(str::trim) {
            // Aliases (`antigravity`) resolve as `--agent` does. trace:STORY-3202 | ai:claude
            found.extend(review_vendor_ext(tool).ok().map(|v| v.program));
        }
    }
    found
}

/// The challenger vendor: an installed one outside `authors`, preferring one
/// other than the finder's. Otherwise the finder's own vendor, with a note.
fn pick_challenger(
    finder: &RunnerCall<'_>,
    authors: &BTreeSet<&str>,
) -> (Option<&'static ReviewVendor>, Option<String>) {
    // trace:BUG-2775 | ai:claude
    let Some(own) = finder.vendor() else {
        let note = finder.runner_cmd_set().then(|| {
            "AIDA_REVIEW_RUNNER_CMD runs both passes (AIDA_REVIEW_PASS=challenger)".to_owned()
        });
        return (finder.agent, note);
    };
    // An unverified vendor (agy) is never picked; only `--challenger-agent`
    // names it. trace:STORY-3202 | ai:claude
    let mut order: Vec<_> = (REVIEW_VENDORS.into_iter())
        .filter(|v| v.confinement_verified)
        .collect();
    order.sort_by_key(|v| v.program == own.program);
    let pick = order
        .into_iter()
        .find(|v| !authors.contains(v.program) && finder.on_path(v).is_some());
    match pick {
        Some(v) if v.program != own.program => (Some(v), None),
        _ => {
            let by: Vec<_> = authors.iter().copied().collect();
            let by = if by.is_empty() {
                "none".into()
            } else {
                by.join(", ")
            };
            let note = format!(
                "no other installed reviewer stays clear of this change's AI authors ({by}); \
                 the challenger is {}, the finder's vendor",
                own.program
            );
            (Some(own), Some(note))
        }
    }
}

/// The challenger's `RunnerCall`: the finder's checkout, environment,
/// deadline and caps, with its own vendor, pass name and validated tuning.
/// `forced` is the operator's `--challenger-agent`: it wins over the pick,
/// so no `[AI:<tool>]` tag in the change can steer it, and is refused when
/// it is not installed.
// trace:STORY-3120 | ai:claude
pub(crate) fn challenger_call<'a>(
    finder: &RunnerCall<'a>,
    authors: &BTreeSet<&str>,
    forced: Option<&'static ReviewVendor>,
    effort_flag: Option<&str>,
    base: &TrustedBase,
) -> Result<(RunnerCall<'a>, AdversarialSummary)> {
    let (agent, note) = match forced {
        Some(v) if finder.vendor().is_some() => {
            let found = finder.on_path(v).is_some();
            ensure!(found, "--challenger-agent {}: not found on PATH", v.program);
            // The finder's own vendor is weaker evidence: say so. trace:STORY-3120 | ai:claude
            let same = finder.vendor().is_some_and(|own| own.program == v.program);
            let note = if same {
                format!(
                    "same-vendor challenger ({}), forced by --challenger-agent",
                    v.program
                )
            } else {
                format!("--challenger-agent picked {}", v.program)
            };
            (Some(v), Some(note))
        }
        Some(_) => bail!("--challenger-agent does not apply to AIDA_REVIEW_RUNNER_CMD"),
        None => pick_challenger(finder, authors),
    };
    let mut call = RunnerCall {
        agent,
        checkout: finder.checkout,
        parent_env: finder.parent_env,
        pass: "challenger",
        deadline: finder.deadline,
        stdout_cap: finder.stdout_cap,
        stderr_cap: finder.stderr_cap,
        tuning: Default::default(),
    };
    call.tuning = resolve_challenger(call.vendor(), finder.parent_env, effort_flag, base)?;
    // Single-vendor opposition fallback: when Finder and Challenger share the same vendor
    // and no explicit challenger effort was configured, automatically step up Challenger
    // reasoning effort by one tier (`low` -> `medium` -> `high` -> `xhigh`, capped at `xhigh`).
    // trace:STORY-3120 | ai:antigravity
    let same_vendor = match (finder.vendor(), call.vendor()) {
        (Some(f), Some(c)) => f.program == c.program,
        _ => false,
    };
    if same_vendor && call.tuning.effort().is_none() {
        let stepped = step_up_challenger_effort(call.vendor(), finder.tuning.effort());
        call.tuning.set_effort_if_none(stepped);
    }
    let summary = AdversarialSummary {
        challenger_agent: agent.map(|v| v.program.to_owned()),
        challenger_effort: call.tuning.effort().map(str::to_owned),
        ai_authors: authors.iter().map(|a| (*a).to_owned()).collect(),
        note,
        ..Default::default()
    };
    Ok((call, summary))
}

/// The challenger prompt: the numbered findings, then the batch's context.
pub(crate) fn challenge_prompt(findings: &[RawReviewFinding], context: &str) -> Result<String> {
    let mut prompt = String::from(CHALLENGE_HEAD);
    for (i, finding) in findings.iter().enumerate() {
        let body = serde_json::to_string_pretty(finding)?;
        prompt.push_str(&format!("## Finding {i}\n```json\n{body}\n```\n\n"));
    }
    prompt.push_str(context);
    Ok(prompt)
}

#[derive(Deserialize)]
struct Verdict {
    index: usize,
    verdict: String,
    #[serde(default)]
    reason: String,
}

#[derive(Deserialize)]
struct Verdicts {
    verdicts: Vec<Verdict>,
}

/// The one verdict object in untrusted challenger output, found by running a
/// JSON parser from each `{` (no slicing, so stray braces cannot panic).
/// None, or a second one, is an error.
// trace:BUG-2779 | ai:claude
fn find_verdicts(raw: &str) -> Result<Verdicts> {
    let parse_at = |at: usize| {
        let mut stream = serde_json::Deserializer::from_str(&raw[at..]).into_iter::<Verdicts>();
        match stream.next() {
            Some(Ok(v)) => Ok((v, at + stream.byte_offset())),
            Some(Err(e)) => Err(e.to_string()),
            None => Err("no JSON value".to_owned()),
        }
    };
    let starts = |from: usize| raw[from..].match_indices('{').map(move |(i, _)| from + i);
    let mut first_error = None;
    for at in starts(0) {
        match parse_at(at) {
            Ok((verdicts, end)) => {
                ensure!(
                    !starts(end).any(|next| parse_at(next).is_ok()),
                    "challenger output holds two verdict objects"
                );
                return Ok(verdicts);
            }
            Err(e) => {
                first_error.get_or_insert(e);
            }
        }
    }
    let cause = first_error.unwrap_or_else(|| "no JSON object".to_owned());
    bail!("challenger output is not verdict JSON: {cause}")
}

/// Keep reproduced findings and set refuted ones aside. A missing,
/// duplicate, out-of-range or unknown verdict refuses the run.
pub(crate) fn apply_verdicts(
    findings: Vec<RawReviewFinding>,
    raw: &str,
) -> Result<(Vec<RawReviewFinding>, Vec<RefutedFinding>)> {
    let parsed = find_verdicts(raw)?;
    let mut slots: Vec<Option<Verdict>> = findings.iter().map(|_| None).collect();
    for verdict in parsed.verdicts {
        let i = verdict.index;
        let slot = slots.get_mut(i);
        let slot = slot.with_context(|| format!("challenger verdict for unknown finding {i}"))?;
        ensure!(slot.is_none(), "challenger gave finding {i} two verdicts");
        *slot = Some(verdict);
    }
    let (mut kept, mut refuted) = (Vec::new(), Vec::new());
    for (i, (finding, slot)) in findings.into_iter().zip(slots).enumerate() {
        let verdict = slot.with_context(|| format!("challenger gave finding {i} no verdict"))?;
        match verdict.verdict.trim().to_ascii_lowercase().as_str() {
            "reproduced" => kept.push(finding),
            "refuted" => refuted.push(RefutedFinding {
                finding,
                reason: verdict.reason,
            }),
            other => {
                bail!("challenger verdict {other:?} for finding {i} is not reproduced|refuted")
            }
        }
    }
    Ok((kept, refuted))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::review_runner::{STDERR_CAP, STDOUT_CAP};
    use crate::vendors::review_runner::review_vendor;
    use std::ffi::OsString;
    use std::time::Duration;

    fn finding(message: &str) -> RawReviewFinding {
        let row =
            serde_json::json!({"path":"a.rs","severity":"high","category":"bug","message":message});
        serde_json::from_value(row).unwrap()
    }

    fn finder<'a>(
        agent: &str,
        checkout: &'a std::path::Path,
        env: &'a [(OsString, OsString)],
    ) -> RunnerCall<'a> {
        RunnerCall {
            agent: review_vendor(agent).ok(),
            checkout,
            parent_env: env,
            pass: "finder",
            deadline: Duration::from_secs(20),
            stdout_cap: STDOUT_CAP,
            stderr_cap: STDERR_CAP,
            tuning: Default::default(),
        }
    }

    #[test]
    fn s4_challenger_refutation_drops_and_reproduction_keeps() {
        let two = || vec![finding("real"), finding("bogus")];
        let out =
            "```json\n{\"verdicts\":[{\"index\":1,\"verdict\":\"Refuted\",\"reason\":\"guarded\"},\
            {\"index\":0,\"verdict\":\"reproduced\"}]}\n```";
        let (kept, refuted) = apply_verdicts(two(), out).unwrap();
        assert_eq!(
            kept.iter().map(|f| f.message.as_str()).collect::<Vec<_>>(),
            ["real"]
        );
        assert_eq!(
            (
                refuted[0].finding.message.as_str(),
                refuted[0].reason.as_str()
            ),
            ("bogus", "guarded")
        );
        let v = |i: usize, verdict: &str| format!("{{\"index\":{i},\"verdict\":\"{verdict}\"}}");
        let refuse = |rows: &[String], want: &str| {
            let raw = format!("{{\"verdicts\":[{}]}}", rows.join(","));
            let err = format!("{:#}", apply_verdicts(two(), &raw).unwrap_err());
            assert!(err.contains(want), "{raw}: {err}");
        };
        refuse(&[v(0, "reproduced")], "finding 1 no verdict");
        refuse(
            &[v(0, "refuted"), v(0, "reproduced"), v(1, "refuted")],
            "two verdicts",
        );
        refuse(&[v(0, "refuted"), v(2, "refuted")], "unknown finding 2");
        refuse(
            &[v(0, "refuted"), v(1, "dismissed")],
            "not reproduced|refuted",
        );
        assert!(apply_verdicts(two(), "no json").is_err());
        assert!(apply_verdicts(Vec::new(), "{\"verdicts\":[]}")
            .unwrap()
            .0
            .is_empty());
    }

    /// Untrusted challenger output never panics: stray, reversed or missing
    /// braces refuse the challenge, and braces inside strings or prose around a
    /// fenced block still parse.
    // trace:BUG-2779 | ai:claude
    #[test]
    fn bug2779_untrusted_challenger_output_fails_closed() {
        let one = || vec![finding("real")];
        for raw in [
            "} sorry {",
            "done} {",
            "}",
            "{",
            "",
            "{}",
            "{\"verdicts\": [",
            "}{\"x\":1}",
        ] {
            let err = format!("{:#}", apply_verdicts(one(), raw).unwrap_err());
            assert!(err.contains("not verdict JSON"), "{raw:?}: {err}");
        }
        let body = r#"{"verdicts":[{"index":0,"verdict":"refuted","reason":"see {x} and }{"}]}"#;
        let shapes = [
            format!("```json\n{body}\n```\nThat is all. }} bye {{"),
            format!("done}} {{ preamble {body} trailing }}"),
            body.to_owned(),
        ];
        for raw in &shapes {
            let (kept, refuted) = apply_verdicts(one(), raw).unwrap();
            assert!(kept.is_empty(), "{raw}");
            assert_eq!(refuted[0].reason, "see {x} and }{", "{raw}");
        }
        let twice = format!("{body}\n{}", body.replace("refuted", "reproduced"));
        let err = format!("{:#}", apply_verdicts(one(), &twice).unwrap_err());
        assert!(err.contains("two verdict objects"), "{err}");
    }

    // trace:BUG-2775 | ai:claude
    #[test]
    fn bug2775_runner_cmd_note_only_when_the_variable_is_set() {
        let checkout = tempfile::tempdir().unwrap();
        let none = BTreeSet::new();
        let unset: Vec<(OsString, OsString)> = vec![("PATH".into(), "/bin".into())];
        let call = RunnerCall {
            agent: None,
            ..finder("claude", checkout.path(), &unset)
        };
        assert_eq!(pick_challenger(&call, &none).1, None);
        let mut set = unset.clone();
        set.push(("AIDA_REVIEW_RUNNER_CMD".into(), "/bin/true".into()));
        let note = pick_challenger(&finder("claude", checkout.path(), &set), &none).1;
        assert!(note
            .unwrap()
            .contains("AIDA_REVIEW_RUNNER_CMD runs both passes"));
    }

    #[test]
    fn s4_detect_ai_authors_from_commit_tags() {
        let log =
            "[AI:claude] feat(x): y (STORY-1)\n\n[ai:Codex+agy:med] fix\n[AI:gemini] z [AI:bogus";
        assert_eq!(
            detect_ai_authors(log).into_iter().collect::<Vec<_>>(),
            ["agy", "claude", "codex"]
        );
        assert!(detect_ai_authors("human commit\n").is_empty());
        // trace:STORY-3202 | ai:claude
        let only = detect_ai_authors("[AI:antigravity] x\n");
        assert_eq!(only.into_iter().collect::<Vec<_>>(), ["agy"]);
    }

    /// The challenger spawns through the S3a contract (empty cwd, cleared env,
    /// stdin prompt, vendor argv) with an opposing vendor where one is installed.
    #[test]
    fn s4_challenger_spawn_contract_and_opposition() {
        let (bin, checkout) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let dump = "#!/bin/sh\necho \"cwd=$(ls -A | tr '\\n' ,)\"; env | sed 's/^/env=/'\n\
            for a in \"$@\"; do echo \"arg=$a\"; done; echo \"stdin=$(wc -c | tr -d ' ')\"\n";
        let install = |name: &str| {
            let src = bin.path().join("src");
            std::fs::write(&src, dump).unwrap();
            let mut cmd = std::process::Command::new("install");
            let ok = cmd
                .arg("-m755")
                .arg(&src)
                .arg(bin.path().join(name))
                .status();
            assert!(ok.unwrap().success());
        };
        install("claude");
        install("codex");
        crate::launch::review_runner::passthrough_bwrap(bin.path());
        let path = format!("{}:/usr/bin:/bin", bin.path().display());
        let env: Vec<(OsString, OsString)> = [
            ("PATH", path.as_str()),
            ("HOME", "/nonexistent"),
            ("GH_TOKEN", "sentinel"),
            ("OPENAI_API_KEY", "k"),
            // The finder's claude tuning: never the codex challenger's.
            ("AIDA_REVIEW_EFFORT", "max"),
            ("AIDA_REVIEW_MODEL", "claude-env-model"),
        ]
        .iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect();
        let mut base = TrustedBase::default();
        base.rules.default_effort = Some("max".into());
        base.rules.default_model = Some("claude-rules-model".into());
        let pick = |agent: &str, authors: &[&'static str], flag| {
            let call = finder(agent, checkout.path(), &env);
            let authors: BTreeSet<&str> = authors.iter().copied().collect();
            let (pass, summary) = challenger_call(&call, &authors, None, flag, &base).unwrap();
            assert_eq!(pass.pass, "challenger");
            (pass.run("x".repeat(200_000).as_str()).unwrap(), summary)
        };
        let (out, summary) = pick("claude", &["claude"], Some("high"));
        assert_eq!(
            (summary.challenger_agent.as_deref(), summary.note),
            (Some("codex"), None)
        );
        assert_eq!(summary.challenger_effort.as_deref(), Some("high"));
        for want in [
            "cwd=\n",
            "env=AIDA_REVIEW_PASS=challenger",
            "env=OPENAI_API_KEY=k",
            "arg=exec",
            "arg=--sandbox=read-only",
            "arg=model_reasoning_effort=high",
            "stdin=200000",
        ] {
            assert!(out.contains(want), "{want}: {out}");
        }
        assert!(!out.contains("sentinel"), "{out}");
        for leaked in [
            "claude-env-model",
            "claude-rules-model",
            "arg=-m",
            "arg=--model",
        ] {
            assert!(!out.contains(leaked), "{leaked}: {out}");
        }
        // No detected author: still the vendor other than the finder's.
        assert_eq!(
            pick("codex", &[], None).1.challenger_agent.as_deref(),
            Some("claude")
        );
        // The opposing vendor is not installed: an explicit fallback note.
        std::fs::remove_file(bin.path().join("codex")).unwrap();
        let (out, summary) = pick("claude", &["claude"], None);
        assert_eq!(summary.challenger_agent.as_deref(), Some("claude"));
        assert!(summary.note.unwrap().contains("AI authors (claude)"));
        assert!(
            out.contains("arg=--print") && !out.contains("OPENAI_API_KEY"),
            "{out}"
        );
    }

    /// `--challenger-agent` beats the pick whatever the commit tags claim,
    /// and an override that is not installed, or under a custom runner, is
    /// refused instead of silently replaced.
    // trace:STORY-3120 | ai:claude
    #[test]
    fn s5c_challenger_agent_override_ignores_forged_tags_and_refuses_missing() {
        let (bin, checkout) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        for name in ["claude", "codex"] {
            crate::test_exec::write_executable(&bin.path().join(name), "#!/bin/sh\n");
        }
        let path = |dir: &std::path::Path| (OsString::from("PATH"), dir.as_os_str().to_owned());
        let env = vec![path(bin.path())];
        let base = TrustedBase::default();
        let codex = review_vendor("codex").ok();
        // A forged `[AI:codex]` would steer the auto pick off codex.
        let forged = detect_ai_authors("[AI:codex] feat: x\n");
        let call = finder("claude", checkout.path(), &env);
        let auto = challenger_call(&call, &forged, None, None, &base)
            .unwrap()
            .1;
        assert_eq!(auto.challenger_agent.as_deref(), Some("claude"));
        let (pass, summary) = challenger_call(&call, &forged, codex, None, &base).unwrap();
        assert_eq!(pass.vendor().map(|v| v.program), Some("codex"));
        assert_eq!(summary.challenger_agent.as_deref(), Some("codex"));
        let note = summary.note.unwrap();
        assert!(note.contains("--challenger-agent"), "{note}");
        assert!(!note.contains("same-vendor"), "{note}");
        // Forcing the finder's own vendor is labelled same-vendor.
        let claude = review_vendor("claude").ok();
        let (_, same) = challenger_call(&call, &forged, claude, None, &base).unwrap();
        assert_eq!(same.challenger_agent.as_deref(), Some("claude"));
        let note = same.note.unwrap();
        assert!(note.contains("same-vendor challenger (claude)"), "{note}");
        // Not installed: refused, never the finder's vendor in its place.
        let only_claude = tempfile::tempdir().unwrap();
        let claude = only_claude.path().join("claude");
        crate::test_exec::write_executable(&claude, "#!/bin/sh\n");
        let env = vec![path(only_claude.path())];
        let call = finder("claude", checkout.path(), &env);
        let Err(err) = challenger_call(&call, &forged, codex, None, &base) else {
            panic!("accepted")
        };
        assert!(err
            .to_string()
            .contains("--challenger-agent codex: not found"));
        let env = vec![(OsString::from("AIDA_REVIEW_RUNNER_CMD"), claude.into())];
        let call = finder("claude", checkout.path(), &env);
        let Err(err) = challenger_call(&call, &forged, codex, None, &base) else {
            panic!("accepted")
        };
        assert!(err.to_string().contains("AIDA_REVIEW_RUNNER_CMD"), "{err}");
        // Outside the allowlist: refused before anything runs.
        assert!(review_vendor("gemini").is_err() && review_vendor("sh").is_err());
    }

    /// agy's confinement is not verified live, so the automatic pick never
    /// chooses it, not even when it is the only other installed vendor; an
    /// explicit `--challenger-agent agy` runs it through the S3a contract with
    /// `--sandbox`, plan mode and the deny-all settings.
    // trace:STORY-3202 | ai:claude
    #[test]
    fn story3202_auto_pick_never_chooses_agy_explicit_override_is_confined() {
        let (bin, checkout) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let dump = "#!/bin/sh\necho \"cwd=$(ls -A)\"; env | sed 's/^/env=/'\n\
            for a in \"$@\"; do echo \"arg=$a\"; done; echo \"stdin=$(wc -c | tr -d ' ')\"\n";
        for name in ["claude", "agy"] {
            crate::test_exec::write_executable(&bin.path().join(name), dump);
        }
        crate::launch::review_runner::passthrough_bwrap(bin.path());
        let path = format!("{}:/usr/bin:/bin", bin.path().display());
        let pairs = [
            ("PATH", path.as_str()),
            ("HOME", "/nonexistent"),
            ("GEMINI_API_KEY", "gem"),
            ("ANTHROPIC_API_KEY", "sentinel"),
        ];
        let env: Vec<(OsString, OsString)> =
            pairs.iter().map(|(k, v)| (k.into(), v.into())).collect();
        let (base, call) = (
            TrustedBase::default(),
            finder("claude", checkout.path(), &env),
        );
        let pick = |tags: &str, forced, effort| {
            challenger_call(&call, &detect_ai_authors(tags), forced, effort, &base).unwrap()
        };
        // Only claude and agy installed, and claude wrote it: still not agy.
        for tags in ["[AI:claude] x", "human commit"] {
            let (pass, summary) = pick(tags, None, None);
            assert_eq!(pass.vendor().map(|v| v.program), Some("claude"), "{tags}");
            assert_eq!(summary.challenger_agent.as_deref(), Some("claude"));
            assert!(summary.note.is_some(), "{tags}");
        }
        assert!(REVIEW_VENDORS
            .iter()
            .any(|v| v.program == "agy" && !v.confinement_verified));
        let agy = review_vendor("agy").ok();
        let (pass, summary) = pick("[AI:claude] x", agy, Some("max"));
        assert_eq!(summary.challenger_agent.as_deref(), Some("agy"));
        let out = pass.run("the prompt").unwrap();
        let want = [
            "cwd=\n",
            "env=GEMINI_API_KEY=gem",
            "arg=--sandbox",
            "arg=--mode=plan",
            "arg=--settings={\"permissions\":{\"deny\":[\"read_file(*)\"",
            "arg=--print=",
            "arg=max",
            "arg=--print-timeout=15s",
            "stdin=10",
        ];
        assert!(
            want.iter().all(|w| out.contains(w)) && !out.contains("sentinel"),
            "{out}"
        );
    }

    #[test]
    fn s4_challenger_effort_injection_refused_at_every_source() {
        let checkout = tempfile::tempdir().unwrap();
        let injected = "high\"\n--model=x";
        let rules = |effort: &str| {
            let mut base = TrustedBase::default();
            base.rules.challenger_effort = Some(effort.into());
            base
        };
        let env_with = |v: &str| {
            vec![(
                OsString::from("AIDA_REVIEW_CHALLENGER_EFFORT"),
                OsString::from(v),
            )]
        };
        let cases = [
            (
                "--challenger-effort",
                Some(injected),
                env_with(""),
                rules("low"),
            ),
            (
                "AIDA_REVIEW_CHALLENGER_EFFORT",
                Some("low"),
                env_with(injected),
                rules("low"),
            ),
            (
                "review-rules.toml challenger_effort",
                Some("low"),
                Vec::new(),
                rules(injected),
            ),
        ];
        for (source, flag, env, base) in cases {
            let call = finder("codex", checkout.path(), &env);
            let Err(err) = challenger_call(&call, &BTreeSet::new(), None, flag, &base) else {
                panic!("{source}: accepted");
            };
            assert!(err.to_string().starts_with(source), "{source}: {err}");
        }
        // The finder's AIDA_REVIEW_EFFORT/MODEL and default_* are not sources.
        let env = vec![
            (OsString::from("AIDA_REVIEW_EFFORT"), OsString::from("max")),
            (
                OsString::from("AIDA_REVIEW_MODEL"),
                OsString::from("claude-x"),
            ),
        ];
        let mut finder_tuned = rules("low");
        finder_tuned.rules.default_effort = Some("max".into());
        finder_tuned.rules.default_model = Some("claude-y".into());
        let codex = review_vendor("codex").ok();
        let tuning = resolve_challenger(codex, &env, Some("high"), &finder_tuned).unwrap();
        assert_eq!(tuning.effort(), Some("high"));
        assert!(!matches!(tuning.model(), Some("claude-x" | "claude-y")));
        // `max` is claude-only: a codex challenger refuses it.
        let env = Vec::new();
        let tuning =
            |agent| resolve_challenger(review_vendor(agent).ok(), &env, Some("max"), &rules("low"));
        assert!(tuning("codex").is_err());
        assert_eq!(tuning("claude").unwrap().effort(), Some("max"));
    }

    // trace:STORY-3120 | ai:antigravity
    #[test]
    fn s9_opencode_pi_no_read_false_and_single_vendor_effort_step_up() {
        for name in ["opencode", "pi"] {
            let v = review_vendor_ext(name).unwrap();
            assert!(!v.no_read, "{name} must have no_read=false");
            assert!(
                !v.confinement_verified,
                "{name} must have confinement_verified=false"
            );
            assert!(!v.post_allowed, "{name} must have post_allowed=false");
        }
        let (bin, checkout) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let dump = "#!/bin/sh\nenv | sed 's/^/env=/'\nfor a in \"$@\"; do echo \"arg=$a\"; done\n";
        for name in ["claude", "codex", "opencode", "pi"] {
            crate::test_exec::write_executable(&bin.path().join(name), dump);
        }
        crate::launch::review_runner::passthrough_bwrap(bin.path());
        let path = format!("{}:/usr/bin:/bin", bin.path().display());
        let env = vec![
            (OsString::from("PATH"), OsString::from(path.as_str())),
            (OsString::from("HOME"), OsString::from("/nonexistent")),
            (
                OsString::from("OPENCODE_CONFIG_CONTENT"),
                OsString::from("{\"tools\":{\"bash\":true}}"),
            ),
            (
                OsString::from("XDG_CONFIG_HOME"),
                checkout.path().join("hostile-xdg").into_os_string(),
            ),
        ];
        let ext_finder = |agent: &str| RunnerCall {
            agent: review_vendor_ext(agent).ok(),
            ..finder("claude", checkout.path(), &env)
        };
        let opencode_out = ext_finder("opencode").run("prompt").unwrap();
        assert!(
            opencode_out.contains("env=OPENCODE_CONFIG_CONTENT={\"tools\":{\"bash\":false")
                && opencode_out.contains("\"permission\":{\"*\":\"deny\"")
                && !opencode_out.contains("\"bash\":true")
                && !opencode_out.contains("XDG_CONFIG_HOME"),
            "{opencode_out}"
        );
        let pi_out = ext_finder("pi").run("prompt").unwrap();
        for flag in [
            "arg=--print",
            "arg=--no-tools",
            "arg=--no-session",
            "arg=--no-extensions",
            "arg=--no-skills",
            "arg=--no-prompt-templates",
        ] {
            assert!(pi_out.contains(flag), "missing {flag} in:\n{pi_out}");
        }
        let claude_call = finder("claude", checkout.path(), &env);
        let detected = BTreeSet::from(["claude", "codex", "opencode", "pi"]);
        // Neither opencode nor pi has confinement_verified=true, so auto-pick never selects them;
        // when claude and codex are both in `detected`, auto-pick falls back to same-vendor claude.
        let (picked, note) = pick_challenger(&claude_call, &detected);
        assert_eq!(picked.map(|v| v.program), Some("claude"));
        assert!(note.is_some());

        let base = TrustedBase::default();
        let codex_vendor = review_vendor("codex").ok();
        for (finder_effort, expected_challenger_effort) in [
            (None, "high"),
            (Some("low"), "medium"),
            (Some("medium"), "high"),
            (Some("high"), "xhigh"),
            (Some("xhigh"), "xhigh"),
        ] {
            let mut f = finder("codex", checkout.path(), &env);
            f.tuning.set_effort_if_none(finder_effort);
            let (challenger, summary) =
                challenger_call(&f, &BTreeSet::from(["codex"]), codex_vendor, None, &base).unwrap();
            assert!(
                summary
                    .note
                    .as_deref()
                    .is_some_and(|n| n.contains("same-vendor challenger")),
                "{summary:?}"
            );
            assert_eq!(challenger.tuning.effort(), Some(expected_challenger_effort));
        }
    }
}
