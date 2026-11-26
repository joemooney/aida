// trace:STORY-3120 | ai:claude
//! `aida review prepare --run-review --adversarial`: after each batch's
//! finder pass, a challenger must reproduce or refute every finding. The
//! challenger is a `RunnerCall` like the finder (same hardened spawn, deadline,
//! caps and trusted-base tuning) and, where one is installed, a vendor that
//! authored none of the change.

use super::review_runner::RunnerCall;
use super::review_tuning::{resolve_challenger, TrustedBase};
use crate::vendors::review_runner::{ReviewVendor, REVIEW_VENDORS};
use aida_core::review_engine::RawReviewFinding;
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const CHALLENGE_HEAD: &str = "You are the challenger in a two-pass code review. \
For each numbered finding below, try to reproduce the defect from the change shown. \
Refute it when the code does not have it. Reply with JSON only, one verdict per \
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
            let named = REVIEW_VENDORS.iter().filter(|v| v.program == tool);
            found.extend(named.map(|v| v.program));
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
    let Some(own) = finder.vendor() else {
        let note = "AIDA_REVIEW_RUNNER_CMD runs both passes (AIDA_REVIEW_PASS=challenger)";
        return (finder.agent, Some(note.into()));
    };
    let mut order = REVIEW_VENDORS.to_vec();
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
/// it is not installed. trace:STORY-3120 | ai:claude
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
            (
                Some(v),
                Some(format!("--challenger-agent picked {}", v.program)),
            )
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

/// Keep reproduced findings and set refuted ones aside. A missing,
/// duplicate, out-of-range or unknown verdict refuses the run.
pub(crate) fn apply_verdicts(
    findings: Vec<RawReviewFinding>,
    raw: &str,
) -> Result<(Vec<RawReviewFinding>, Vec<RefutedFinding>)> {
    let span = raw.find('{').zip(raw.rfind('}'));
    let payload = span.map_or(raw, |(start, end)| &raw[start..=end]);
    let parsed: Verdicts =
        serde_json::from_str(payload).context("challenger output is not verdict JSON")?;
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

    #[test]
    fn s4_detect_ai_authors_from_commit_tags() {
        let log =
            "[AI:claude] feat(x): y (STORY-1)\n\n[ai:Codex+agy:med] fix\n[AI:gemini] z [AI:bogus";
        assert_eq!(
            detect_ai_authors(log).into_iter().collect::<Vec<_>>(),
            ["claude", "codex"]
        );
        assert!(detect_ai_authors("human commit\n").is_empty());
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
        let path = format!("{}:/usr/bin:/bin", bin.path().display());
        let env: Vec<(OsString, OsString)> = [
            ("PATH", path.as_str()),
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
    /// refused instead of silently replaced. trace:STORY-3120 | ai:claude
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
        assert!(summary.note.unwrap().contains("--challenger-agent"));
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
}
