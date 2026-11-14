//! `aida integration`: the integration-branch flow (STORY-2737). Work lands
//! on `integration` with minimal ceremony; `main` is the batched,
//! fully-verified checkpoint that completes specs.
//!
//! `aida integration tags` keeps the `landed-in-integration` tag honest so
//! the open list shows which specs are done but not yet promoted. It reads
//! commit subjects with the same trailer parser merge-time completion uses,
//! so a `(Refs X)` commit never marks `X` landed. Writes go through
//! `aida edit --add-tag/--remove-tag`, the one validated tag-write path.
// trace:STORY-2737 | ai:claude

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[path = "integration_cmd_args.rs"]
pub(crate) mod integration_cmd_args;
pub(crate) use integration_cmd_args::*;
#[path = "integration_review.rs"]
pub(crate) mod review;

/// The tag on a spec whose completing commit is on integration, not main.
pub(crate) const LANDED_TAG: &str = "landed-in-integration";

/// Run `git` in `root`, returning trimmed stdout on success.
fn git(root: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .current_dir(root)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    anyhow::ensure!(
        out.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// The spec ids the commit `subjects` complete, upper-cased and deduplicated
/// only as literal tokens, in first-seen order. Padding-equal tokens
/// (`BUG-9`, `BUG-09`) stay distinct: each may name its own stored spec, so
/// identity is deduplicated after [`resolve_trailer`], by UUID.
// trace:STORY-2737 | ai:claude
pub(crate) fn completing_ids(subjects: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for subject in subjects {
        for id in crate::zen_drive_root::extract_spec_ids_from_commit(subject) {
            let id = id.to_ascii_uppercase();
            if !out.contains(&id) {
                out.push(id);
            }
        }
    }
    out
}

/// One stored spec, as the tag plan sees it: its stable identity (the
/// UUID `aida edit` resolves directly), the id shown to the operator, its
/// status and tags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ListedSpec {
    pub uuid: uuid::Uuid,
    pub display_id: String,
    pub status: aida_core::RequirementStatus,
    pub tags: Vec<String>,
}

impl ListedSpec {
    fn of(req: &aida_core::Requirement) -> Self {
        let mut tags: Vec<String> = req.tags.iter().cloned().collect();
        tags.sort();
        Self {
            uuid: req.id,
            display_id: req.display_id(), // canonical-id: sort key; printed through display_spec_id
            status: req.status.clone(),
            tags,
        }
    }
}

/// Whether a spec in `status` can still be awaiting promotion to main. Done
/// is the normal state of implemented work whose completing commit is on
/// integration only, so it keeps (and gains) the tag until promotion
/// completes it.
// trace:STORY-2737 | ai:claude
fn awaits_promotion(status: &aida_core::RequirementStatus) -> bool {
    use aida_core::RequirementStatus as S;
    matches!(
        status,
        S::Draft | S::Approved | S::Planned | S::InProgress | S::NeedsAttention | S::Done
    )
}

/// The stored spec a commit trailer token names: the exact literal id
/// (native `spec_id`, then agreed id, compared as strings, never by numeric
/// identity), then its node-alias expansion, and a numeric-padding match
/// (`TASK-0042`) last, only when it is unique. Origin ids are native
/// `spec_id`s, so a pre-agreement trailer resolves to the same spec as its
/// agreed display id. `None` for a token naming no spec, or one whose
/// padding matches two stored specs.
// trace:STORY-2737 | ai:claude
pub(crate) fn resolve_trailer(
    store: &aida_core::RequirementsStore,
    token: &str,
) -> Option<uuid::Uuid> {
    let literal = |id: &str| {
        let exact = |field: &Option<String>| {
            field
                .as_deref()
                .is_some_and(|stored| stored.eq_ignore_ascii_case(id))
        };
        let native = store.requirements.iter().find(|r| exact(&r.spec_id));
        native
            .or_else(|| store.requirements.iter().find(|r| exact(&r.agreed_id)))
            .map(|r| r.id)
    };
    aida_core::node_alias::resolve_literal_first_opt(token, literal).or_else(|| {
        let mut hits = store.requirements.iter().filter(|r| {
            // canonical-id: matched against the token, never printed
            [Some(r.display_id()), r.spec_id.clone(), r.agreed_id.clone()] // canonical-id: see above
                .into_iter()
                .flatten()
                .any(|id| aida_core::id_identity::same_id(&id, token))
        });
        // Padding-equal ids that name two specs are ambiguous: tag neither.
        match (hits.next(), hits.next()) {
            (Some(one), None) => Some(one.id),
            _ => None,
        }
    })
}

/// The tag edits that make `specs` agree with `landed` (the stored specs
/// completed on integration but not on main): add the tag to every open
/// landed spec that lacks it; remove it from every tagged spec that is
/// closed or not landed. Returned sorted by display id, as (adds, removes).
pub(crate) fn plan_tag_changes<'a>(
    specs: &'a [ListedSpec],
    landed: &std::collections::HashSet<uuid::Uuid>,
) -> (Vec<&'a ListedSpec>, Vec<&'a ListedSpec>) {
    let mut adds = Vec::new();
    let mut removes = Vec::new();
    for spec in specs {
        let tagged = spec.tags.iter().any(|t| t == LANDED_TAG);
        let wanted = awaits_promotion(&spec.status) && landed.contains(&spec.uuid);
        if wanted && !tagged {
            adds.push(spec);
        } else if tagged && !wanted {
            removes.push(spec);
        }
    }
    adds.sort_by(|a, b| a.display_id.cmp(&b.display_id));
    removes.sort_by(|a, b| a.display_id.cmp(&b.display_id));
    (adds, removes)
}

/// Every stored spec, read from the store itself: no list query, so no
/// role scope, focus or status filter can hide a landed or tagged spec.
// trace:STORY-2737 | ai:claude
pub(crate) fn load_store(root: &Path) -> Result<aida_core::RequirementsStore> {
    let path = aida_core::store_locate::detect_distributed_store_from(root)
        .unwrap_or_else(|| root.join(".aida-store"));
    aida_core::Storage::new(path)
        .load()
        .context("loading the requirements store")
}

fn run_tags(root: &Path, dry_run: bool) -> Result<()> {
    for r in [
        "refs/remotes/origin/main",
        "refs/remotes/origin/integration",
    ] {
        git(root, &["rev-parse", "--verify", "-q", r])
            .with_context(|| format!("`{r}` is missing; run `git fetch origin` first"))?;
    }
    let log = git(
        root,
        &[
            "log",
            "--format=%s",
            "refs/remotes/origin/main..refs/remotes/origin/integration",
        ],
    )?;
    let subjects: Vec<&str> = log.lines().collect();
    let landed = completing_ids(&subjects);
    let store = load_store(root)?;
    let landed_specs: std::collections::HashSet<uuid::Uuid> = landed
        .iter()
        .filter_map(|token| resolve_trailer(&store, token))
        .collect();
    let specs: Vec<ListedSpec> = store.requirements.iter().map(ListedSpec::of).collect();
    let (adds, removes) = plan_tag_changes(&specs, &landed_specs);
    println!(
        "{} commit(s) on origin/integration not on origin/main complete {} spec(s).",
        subjects.len(),
        landed_specs.len()
    );
    if adds.is_empty() && removes.is_empty() {
        println!("`{LANDED_TAG}` tags are up to date.");
        return Ok(());
    }
    for (verb, flag, specs) in [
        ("tag", "--add-tag", &adds),
        ("untag", "--remove-tag", &removes),
    ] {
        for spec in specs {
            let id = aida_core::node_alias::display_spec_id(&spec.display_id);
            if dry_run {
                println!("  would {verb} {id}");
                continue;
            }
            // Edit by UUID: the stable identity, never an alias guess.
            let uuid = spec.uuid.to_string();
            let status = Command::new(crate::kernel::aida_exe_path())
                .current_dir(root)
                .args(["edit", &uuid, flag, LANDED_TAG])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .status()
                .with_context(|| format!("running aida edit {id}"))?;
            anyhow::ensure!(
                status.success(),
                "aida edit {id} {flag} {LANDED_TAG} failed"
            );
            println!("  {verb}ged {id}");
        }
    }
    Ok(())
}

/// Add or remove `tag` on the spec `uuid` through `aida edit`, the one
/// validated tag-write path.
fn edit_tag(root: &Path, uuid: &uuid::Uuid, flag: &str, tag: &str) -> Result<()> {
    let status = Command::new(crate::kernel::aida_exe_path())
        .current_dir(root)
        .args(["edit", &uuid.to_string(), flag, tag])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .context("running aida edit")?;
    anyhow::ensure!(status.success(), "aida edit {flag} {tag} failed");
    Ok(())
}

/// Merge `head` into the checked-out branch with `subject` and `body`, then
/// run each of `checks` on the result. A merge that stops, a merge of some
/// other commit, or a failing check puts the branch back at the commit it
/// started from and is an error.
// trace:STORY-2772 | ai:claude
pub(crate) fn merge_checked(
    root: &Path,
    head: &str,
    subject: &str,
    body: &str,
    checks: &[String],
) -> Result<String> {
    let before = git(root, &["rev-parse", "HEAD"])?;
    let undo = |why: String| -> anyhow::Error {
        let _ = git(root, &["merge", "--abort"]);
        match git(root, &["reset", "--hard", &before]) {
            Ok(_) => anyhow::anyhow!("{why}; `integration` is back at {}", &before[..11]),
            Err(error) => anyhow::anyhow!(
                "{why}; and the branch could not be put back at {before}: {error:#}"
            ),
        }
    };
    if let Err(error) = git(
        root,
        &[
            "merge",
            "--no-ff",
            "--no-edit",
            "-m",
            subject,
            "-m",
            body,
            head,
        ],
    ) {
        return Err(undo(format!("the merge did not complete ({error:#})")));
    }
    match git(root, &["rev-parse", "HEAD^2"]) {
        Ok(second) if second == head => {}
        _ => return Err(undo("the merge did not record the change's head".into())),
    }
    for check in checks {
        println!("check: {check}");
        let passed = Command::new("sh")
            .current_dir(root)
            .args(["-c", check])
            .stdin(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !passed {
            return Err(undo(format!("`{check}` failed on the merge result")));
        }
    }
    git(root, &["rev-parse", "HEAD"])
}

fn run_merge(root: &Path, pr: u64, push: bool, dry_run: bool) -> Result<()> {
    use review::Admission;
    let root = PathBuf::from(git(root, &["rev-parse", "--show-toplevel"])?);
    let root = root.as_path();
    let branch = git(root, &["symbolic-ref", "--short", "-q", "HEAD"]).unwrap_or_default();
    anyhow::ensure!(
        branch == crate::land::INTEGRATION_BRANCH,
        "this checkout is on `{}`, not `integration`",
        if branch.is_empty() {
            "a detached HEAD"
        } else {
            &branch
        }
    );
    anyhow::ensure!(
        git(root, &["status", "--porcelain", "--untracked-files=no"])?.is_empty(),
        "tracked changes are present; commit or discard them first"
    );
    git(root, &["fetch", "-q", "origin", "integration"])?;
    anyhow::ensure!(
        git(root, &["rev-parse", "HEAD"])? == git(root, &["rev-parse", "origin/integration"])?,
        "local `integration` is not `origin/integration`; pull or push first"
    );

    let pass = crate::land::live_pass(root)?;
    let found = pass
        .changes
        .iter()
        .zip(&pass.rows)
        .find(|(change, _)| change.id == pr);
    let Some((change, row)) = found else {
        anyhow::bail!("#{pr} is not an open change");
    };
    let reader = review::Reader::new(&pass.root);
    let config = reader.config().map_err(anyhow::Error::msg)?.clone();
    let hold = crate::land::hold_of(&pass.root, change).map(|hold| hold.map(|hold| hold.detail));
    let admission = review::admit(row, hold, || {
        let paths = crate::land::changed_paths(root, "origin/integration", &change.head)?;
        reader.eligibility(&crate::land::specs_of(change), &paths)
    })
    .map_err(|why| anyhow::anyhow!("#{pr} is not merged: {why}"))?;
    let (how, body) = match &admission {
        Admission::Approved(why) => (
            format!("approved ({why})"),
            format!("Approved at the head {}: {why}. No merge hold.", change.head),
        ),
        Admission::ReviewAfterMerge(why) => (
            format!("review after merge: {why}"),
            format!(
                "Review after merge: {why}. Merged head {}; a promotion to main needs its approval at that head.",
                change.head
            ),
        ),
    };
    println!("#{pr} admitted: {how}");
    if dry_run {
        println!("(dry run) nothing merged.");
        return Ok(());
    }
    let subject = review::merge_subject(pr, &change.title);
    let tip = merge_checked(root, &change.head, &subject, &body, &config.checks)?;
    println!("merged #{pr} at {}", &tip[..11]);
    if matches!(admission, Admission::ReviewAfterMerge(_)) {
        let specs = reader
            .resolve(&crate::land::specs_of(change))
            .map_err(anyhow::Error::msg)?;
        for (id, spec) in specs {
            let Some(spec) = spec else { continue };
            if spec.tags.contains(review::MERGED_BEFORE_REVIEW_TAG) {
                continue;
            }
            match edit_tag(
                root,
                &spec.id,
                "--add-tag",
                review::MERGED_BEFORE_REVIEW_TAG,
            ) {
                Ok(()) => println!("  tagged {id} `{}`", review::MERGED_BEFORE_REVIEW_TAG),
                // The tag is what the open list shows; the promotion check
                // reads the merge itself and does not need it.
                Err(error) => eprintln!("  {id} was not tagged: {error:#}"),
            }
        }
    }
    if push {
        git(root, &["push", "origin", "HEAD:refs/heads/integration"])?;
        println!("pushed `integration` at {}", &tip[..11]);
    } else {
        println!("not pushed (pass --push, or `git push origin integration`).");
    }
    Ok(())
}

fn run_promotion_check(root: &Path, tip: Option<&str>, since: Option<&str>) -> Result<()> {
    use review::FirstParent;
    let main = "refs/remotes/origin/main";
    let tip = tip.unwrap_or("refs/remotes/origin/integration");
    for r in [main, tip] {
        git(
            root,
            &["rev-parse", "--verify", "-q", &format!("{r}^{{commit}}")],
        )
        .with_context(|| format!("`{r}` is not a commit here; run `git fetch origin` first"))?;
    }
    let main_trees: std::collections::HashSet<String> = git(
        root,
        &["log", "--first-parent", "-n", "500", "--format=%T", main],
    )?
    .lines()
    .map(String::from)
    .collect();
    let log = git(
        root,
        &[
            "log",
            "--first-parent",
            "--format=%H%x1f%P%x1f%T%x1f%s",
            &format!("{main}..{tip}"),
        ],
    )?;
    let commits: Vec<FirstParent> = log
        .lines()
        .filter_map(|line| {
            let mut fields = line.splitn(4, '\x1f');
            Some(FirstParent {
                sha: fields.next()?.to_string(),
                parents: fields
                    .next()?
                    .split_whitespace()
                    .map(String::from)
                    .collect(),
                tree: fields.next()?.to_string(),
                subject: fields.next()?.to_string(),
            })
        })
        .collect();
    let commits = match since {
        Some(since) => {
            let full = git(
                root,
                &[
                    "rev-parse",
                    "--verify",
                    "-q",
                    &format!("{since}^{{commit}}"),
                ],
            )
            .with_context(|| format!("`{since}` is not a commit here"))?;
            review::after(&commits, &full).with_context(|| {
                format!("`{since}` is not on the first-parent line of `{tip}` after `main`")
            })?
        }
        None => {
            let left = review::unpromoted(&commits, &main_trees);
            // Squashed promotions leave no ancestry: without `--since` the
            // boundary is inferred, and says so.
            println!(
                "No --since given: the last promotion's cut is a guess ({}). Pass --since <commit> to state it.",
                match commits.get(left.len()) {
                    Some(cut) => format!(
                        "{}, the newest integration commit whose content is a commit on main",
                        &cut.sha[..11.min(cut.sha.len())]
                    ),
                    None => "none found, so every commit not on main is checked".to_string(),
                }
            );
            left
        }
    };
    let ledger_root = crate::review_watch::forge::main_root(root)?;
    let on_main = |sha: &str| {
        Command::new("git")
            .current_dir(root)
            .args(["merge-base", "--is-ancestor", sha, main])
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .stdin(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    };
    let verdict = |pr: u64| {
        crate::gate::review_verdict::read_recorded_verdict(&ledger_root, &format!("PR-{pr}")).map(
            |verdict| review::Verdict {
                approves: verdict.kind.approves(),
                label: verdict.kind.label().to_string(),
                sha: verdict.reviewed_sha,
            },
        )
    };
    let (approved, findings) = review::promotion_check(commits, &on_main, &verdict);
    println!(
        "{} commit(s) on integration are not yet promoted; {} merged change(s) approved at the merged head.",
        commits.len(),
        approved.len()
    );
    // An approved change no longer needs its marker in the open list.
    if let Ok(store) = load_store(root) {
        for constituent in &approved {
            for token in crate::pr_ship::extract_spec_ids_from_text(&constituent.subject) {
                let spec = resolve_trailer(&store, &token)
                    .and_then(|uuid| store.requirements.iter().find(|r| r.id == uuid));
                let Some(spec) = spec else { continue };
                if spec.tags.contains(review::MERGED_BEFORE_REVIEW_TAG) {
                    let id = aida_core::node_alias::display_spec_id(&token);
                    match edit_tag(
                        root,
                        &spec.id,
                        "--remove-tag",
                        review::MERGED_BEFORE_REVIEW_TAG,
                    ) {
                        Ok(()) => println!("  untagged {id}"),
                        Err(error) => eprintln!("  {id} was not untagged: {error:#}"),
                    }
                }
            }
        }
    }
    if findings.is_empty() {
        println!("Promotion allowed.");
        return Ok(());
    }
    for finding in &findings {
        println!("  {}", finding.describe());
    }
    anyhow::bail!(
        "promotion refused: {} commit(s) on integration are not reviewed at the merged head",
        findings.len()
    )
}

/// The integration-branch flow: `integration` takes work fast; `main` is the
/// batched checkpoint that completes specs.
// trace:STORY-2737 | ai:claude
#[derive(clap::Parser, Debug)]
#[clap(name = "integration")]
pub(crate) struct IntegrationArgs {
    #[clap(subcommand)]
    pub command: IntegrationCommand,
}

fn run_integration(matches: &clap::ArgMatches, _ctx: &crate::cmdkit::DispatchCtx) -> Result<()> {
    let args = <IntegrationArgs as clap::FromArgMatches>::from_arg_matches(matches)?;
    let root = std::env::current_dir().context("reading the current directory")?;
    match args.command {
        IntegrationCommand::Tags { dry_run } => run_tags(&root, dry_run),
        IntegrationCommand::Merge { pr, push, dry_run } => run_merge(&root, pr, push, dry_run),
        IntegrationCommand::PromotionCheck { tip, since } => {
            run_promotion_check(&root, tip.as_deref(), since.as_deref())
        }
    }
}

// The tag writes go through `aida edit`, which carries its own policy.
// trace:STORY-2737 | ai:claude
pub(crate) static INTEGRATION: crate::cmdkit::CommandReg = crate::cmdkit::CommandReg {
    name: "integration",
    help_after: "integrate",
    command: <IntegrationArgs as clap::CommandFactory>::command,
    policy: |_| crate::cmdkit::CmdPolicy {
        stakeholder: crate::stakeholder_role::StakeholderAction::Write,
        per_write_auto_push: false,
        skip_role_restore_prompt: false,
        team_identity_write: false,
    },
    run: crate::cmdkit::Handler::Early(run_integration),
};

#[cfg(test)]
#[path = "tests/story_2737_integration_tags_tests.rs"]
mod tests;
