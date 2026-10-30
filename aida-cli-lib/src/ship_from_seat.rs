//! `aida ship --from-seat <clone>`: the host publishes a contained seat's
//! committed branch without running git in the seat's clone (BUG-HOME-038).
//!
//! A seat clone's `.git/config` and hooks are seat-writable, so host git
//! never runs with the clone as its repository or as a remote (the TASK-2053
//! invariant). Instead, per the advisor's ruling:
//!
//!   1. the clone must be one this checkout recorded (its `clone` lease);
//!   2. git inside the seat sandbox bundles `refs/heads/<branch>` alone
//!      ([`crate::seat_clone::bundle_branch`]: hooks and fsmonitor off);
//!   3. the host runs `git bundle verify` and accepts exactly one ref, the
//!      lease's branch, whose prerequisites the host already has;
//!   4. the bundle is fetched into `refs/aida/seat-import/<branch>`, never
//!      into `refs/heads`; the diff stat against the default branch and the
//!      clone's config/hooks report are printed, and the STORY-469 trailer
//!      check runs on the imported commits. A dry run fetches into a scratch
//!      repository instead and stops here, leaving no ref in main;
//!   5. the quarantined commit is pushed to `origin/<branch>`, the quarantine
//!      ref is deleted, and the PR is opened, titled with the commit subject.
//!
//! trace:BUG-HOME-038 | ai:claude

use std::path::{Path, PathBuf};

use aida_core::git_ops::RemoteCredentialsExt as _;
use anyhow::{Context, Result};
use colored::Colorize as _;

use crate::seat_clone::{HostRecordedClone, TreeKind};

/// Where an imported seat branch waits before it is pushed.
// trace:BUG-HOME-038 | ai:claude
pub(crate) const QUARANTINE_PREFIX: &str = "refs/aida/seat-import/";

/// `aida ship --from-seat` options.
// trace:BUG-HOME-038 | ai:claude
pub(crate) struct FromSeatOptions {
    /// The seat clone to publish from.
    pub(crate) clone: PathBuf,
    /// Explicit spec; `None` resolves it from the lease's branch or scope.
    pub(crate) spec: Option<String>,
    /// Push without opening the PR.
    pub(crate) no_pr: bool,
    /// Verify and report in a scratch repository; import and push nothing.
    pub(crate) dry_run: bool,
}

/// `clone` as a seat clone `main` recorded, or the refusal.
// trace:BUG-HOME-038 | ai:claude
pub(crate) fn recorded_seat(main: &Path, clone: &Path) -> Result<HostRecordedClone> {
    aida_core::seat_clone_detect::note_host_main(main);
    let main = main.canonicalize().unwrap_or_else(|_| main.to_path_buf());
    crate::seat_clone::host_recorded_clone(clone)
        .filter(|seat| seat.main == main)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "refusing to publish from {}: it is not a seat clone this checkout recorded \
                 (no `clone` session lease in {} names it)",
                clone.display(),
                main.display()
            )
        })
}

/// The branch and scope of `seat`'s `clone` lease, read from the host-side
/// lease, never from the clone. The branch must be a plain branch name.
// trace:BUG-HOME-038 | ai:claude
fn lease_branch(seat: &HostRecordedClone) -> Result<(String, String)> {
    let lease = crate::list_leases(&seat.main)
        .into_iter()
        .find(|lease| {
            lease.tree_kind == TreeKind::Clone
                && lease
                    .worktree_path
                    .canonicalize()
                    .is_ok_and(|p| p == seat.root)
        })
        .with_context(|| format!("the lease for {} disappeared", seat.root.display()))?;
    crate::git_arg_guard::reject_option_like("seat branch", &lease.branch)?;
    let valid = aida_core::git_ops::host_git_command(&seat.main)
        .args(["check-ref-format", "--branch", &lease.branch])
        .output()
        .is_ok_and(|o| o.status.success());
    if !valid || lease.branch.is_empty() {
        anyhow::bail!(
            "the seat's lease names `{}`, which is not a valid branch name",
            lease.branch.escape_debug()
        );
    }
    Ok((lease.branch, lease.scope))
}

/// The one head `git bundle list-heads` printed, when it is exactly
/// `refs/heads/<branch>` at a full object id; the refusal otherwise.
// trace:BUG-HOME-038 | ai:claude
pub(crate) fn single_branch_head(list_heads: &str, branch: &str) -> Result<String> {
    let heads: Vec<&str> = list_heads
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect();
    let want = format!("refs/heads/{branch}");
    let [line] = heads.as_slice() else {
        anyhow::bail!(
            "refusing the seat bundle: it carries {} refs, and only `{want}` is accepted",
            heads.len()
        );
    };
    let (oid, name) = line
        .split_once(' ')
        .context("refusing the seat bundle: its head list is malformed")?;
    let full_hex = matches!(oid.len(), 40 | 64) && oid.bytes().all(|b| b.is_ascii_hexdigit());
    if name != want || !full_hex {
        anyhow::bail!(
            "refusing the seat bundle: it carries `{}`, and only `{want}` is accepted",
            name.escape_debug()
        );
    }
    Ok(oid.to_string())
}

/// `git bundle verify` in `main` (every prerequisite must already be there),
/// then the one-ref check. Returns the branch tip.
// trace:BUG-HOME-038 | ai:claude
pub(crate) fn verify_bundle(main: &Path, bundle: &Path, branch: &str) -> Result<String> {
    let verify = aida_core::git_ops::host_git_command(main)
        .args(["bundle", "verify", "--quiet"])
        .arg(bundle)
        .output()
        .context("could not run `git bundle verify`")?;
    if !verify.status.success() {
        anyhow::bail!(
            "refusing the seat bundle: `git bundle verify` failed (are its prerequisites in \
             this repository?): {}",
            String::from_utf8_lossy(&verify.stderr).trim()
        );
    }
    let heads = aida_core::git_ops::host_git_command(main)
        .args(["bundle", "list-heads"])
        .arg(bundle)
        .output()
        .context("could not run `git bundle list-heads`")?;
    if !heads.status.success() {
        anyhow::bail!(
            "refusing the seat bundle: `git bundle list-heads` failed: {}",
            String::from_utf8_lossy(&heads.stderr).trim()
        );
    }
    single_branch_head(&String::from_utf8_lossy(&heads.stdout), branch)
}

/// Fetch a verified bundle's branch into its quarantine ref in `main` and
/// check the ref landed on `tip`. Returns the quarantine ref.
// trace:BUG-HOME-038 | ai:claude
pub(crate) fn quarantine_bundle(
    main: &Path,
    bundle: &Path,
    branch: &str,
    tip: &str,
) -> Result<String> {
    let quarantine = format!("{QUARANTINE_PREFIX}{branch}");
    let fetch = aida_core::git_ops::host_git_command(main)
        .args(["fetch", "--quiet", "--no-tags", "--no-write-fetch-head"])
        .arg(bundle)
        .arg(format!("+refs/heads/{branch}:{quarantine}"))
        .output()
        .context("could not run `git fetch` from the seat bundle")?;
    if !fetch.status.success() {
        anyhow::bail!(
            "could not fetch the seat bundle into {quarantine}: {}",
            String::from_utf8_lossy(&fetch.stderr).trim()
        );
    }
    let landed = git_text(main, &["rev-parse", "--verify", &quarantine])?;
    if landed != tip {
        anyhow::bail!("{quarantine} is at {landed}, not the bundle's {tip}");
    }
    Ok(quarantine)
}

/// Host git in `main`, stdout trimmed; an error carries stderr.
fn git_text(main: &Path, args: &[&str]) -> Result<String> {
    let out = aida_core::git_ops::host_git_command(main)
        .args(args)
        .output()
        .with_context(|| format!("could not run `git {}`", args.join(" ")))?;
    if !out.status.success() {
        anyhow::bail!(
            "`git {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// A verified seat branch fetched into a quarantine ref: in main for a real
/// run, or in a scratch repository borrowing main's objects for a dry run, so
/// a dry run leaves no ref in main.
// trace:BUG-HOME-038 | ai:claude
pub(crate) struct Staged {
    /// The repository holding `reference`.
    pub(crate) repo: PathBuf,
    /// The quarantine ref.
    pub(crate) reference: String,
    /// The branch tip the bundle carried.
    pub(crate) tip: String,
    /// The dry run's scratch repository, removed on drop.
    _scratch: Option<tempfile::TempDir>,
}

/// Verify the seat's bundle `bytes` against `main`, then fetch it into
/// quarantine: main's `refs/aida/seat-import/<branch>`, or with `dry_run` the
/// same ref in a scratch bare repository whose alternates borrow main's
/// objects (`git bundle verify` and that fetch write nothing to main).
// trace:BUG-HOME-038 | ai:claude
pub(crate) fn stage_bundle(
    main: &Path,
    branch: &str,
    bytes: &[u8],
    dry_run: bool,
) -> Result<Staged> {
    let mut bundle = tempfile::Builder::new()
        .prefix("aida-from-seat-")
        .suffix(".bundle")
        .tempfile()
        .context("could not create a temporary bundle file")?;
    std::io::Write::write_all(&mut bundle, bytes).context("could not write the seat bundle")?;
    let tip = verify_bundle(main, bundle.path(), branch)?;
    if !dry_run {
        let reference = quarantine_bundle(main, bundle.path(), branch, &tip)?;
        return Ok(Staged {
            repo: main.to_path_buf(),
            reference,
            tip,
            _scratch: None,
        });
    }
    let scratch = tempfile::Builder::new()
        .prefix("aida-from-seat-dry-run-")
        .tempdir()
        .context("could not create the dry run's scratch repository")?;
    let init = aida_core::git_ops::host_git_command(scratch.path())
        .args(["init", "--bare", "--quiet"])
        .output()
        .context("could not run `git init` for the dry run")?;
    if !init.status.success() {
        anyhow::bail!(
            "could not create the dry run's scratch repository: {}",
            String::from_utf8_lossy(&init.stderr).trim()
        );
    }
    let objects = main
        .join(".git")
        .join("objects")
        .canonicalize()
        .context("could not resolve main's object directory")?;
    std::fs::write(
        scratch
            .path()
            .join("objects")
            .join("info")
            .join("alternates"),
        format!("{}\n", objects.display()),
    )
    .context("could not point the dry run's scratch repository at main's objects")?;
    let reference = quarantine_bundle(scratch.path(), bundle.path(), branch, &tip)?;
    Ok(Staged {
        repo: scratch.path().to_path_buf(),
        reference,
        tip,
        _scratch: Some(scratch),
    })
}

/// Import a recorded seat's branch into main's quarantine: bundle it in the
/// seat sandbox, verify it, fetch it. Returns the quarantine ref and the tip.
// trace:BUG-HOME-038 | ai:claude
pub(crate) fn import_seat_branch(
    seat: &HostRecordedClone,
    branch: &str,
) -> Result<(String, String)> {
    let bytes = crate::seat_clone::bundle_branch(&seat.main, &seat.root, branch)?;
    let staged = stage_bundle(&seat.main, branch, &bytes, false)?;
    Ok((staged.reference, staged.tip))
}

/// The STORY-469 commit-trailer check on the seat's commits in `range`
/// (read in `repo`): some commit carries a `(SPEC-ID)` trailer, and every
/// trailer resolves to a live spec through `resolve`. A seat branch with no
/// commit in `range` has nothing to publish.
// trace:BUG-HOME-038 trace:STORY-469 | ai:claude
pub(crate) fn seat_trailer_check<F>(repo: &Path, range: &str, resolve: F) -> Result<()>
where
    F: FnMut(&str) -> crate::trace::SpecResolution,
{
    let commits = crate::trace::read_commits_in_range(repo, range)?;
    if commits.is_empty() {
        anyhow::bail!("refusing to publish: the seat branch adds no commit ({range})");
    }
    let trailered = commits
        .iter()
        .any(|(_, subject)| !crate::extract_spec_ids_from_commit(subject).is_empty());
    if !trailered {
        anyhow::bail!(
            "refusing to publish: no seat commit carries a `(SPEC-ID)` trailer. Amend the \
             commit inside the seat, then publish again"
        );
    }
    let violations = crate::trace::validate_trailer_references(&commits, resolve);
    if !violations.is_empty() {
        anyhow::bail!(
            "{}",
            crate::trace::format_trailer_guard_refusal("ship --from-seat", &violations).join("\n")
        );
    }
    Ok(())
}

/// What `aida ship --from-seat` needs from outside the repository, so a test
/// can drive the handler without bwrap, a forge or a requirement store.
// trace:BUG-HOME-038 | ai:claude
pub(crate) struct Deps<'a> {
    /// Bundle `refs/heads/<branch>` of the clone (`main`, `clone`, `branch`).
    pub(crate) bundle: &'a dyn Fn(&Path, &Path, &str) -> Result<Vec<u8>>,
    /// The forge to open the PR on, for `main`.
    pub(crate) forge: &'a dyn Fn(&Path) -> Box<dyn crate::forge::Forge>,
    /// Called with `main` and the branch once the PR is open.
    pub(crate) after_open: &'a dyn Fn(&Path, &str),
}

/// The `aida ship --from-seat` handler.
// trace:BUG-HOME-038 | ai:claude
pub(crate) fn run(opts: FromSeatOptions) -> Result<()> {
    let cwd = std::env::current_dir().context("could not read the current directory")?;
    run_from(
        &cwd,
        opts,
        &Deps {
            bundle: &crate::seat_clone::bundle_branch,
            forge: &|main| crate::forge::forge_for_open_change(main),
            after_open: &|main, branch| {
                let _ = crate::queue_review_for_open_pr(main, branch);
            },
        },
    )
}

/// [`run`] from `cwd`.
// trace:BUG-HOME-038 | ai:claude
pub(crate) fn run_from(cwd: &Path, opts: FromSeatOptions, deps: &Deps<'_>) -> Result<()> {
    if let Some(clone) = aida_core::seat_clone_detect::seat_clone_root(cwd) {
        anyhow::bail!(
            "`aida ship --from-seat` runs from the main checkout, not from inside the seat \
             clone {}",
            clone.display()
        );
    }
    let project_root = crate::find_project_root_from(cwd)?;
    let main = crate::main_worktree_root_from(&project_root);
    let seat = recorded_seat(&main, &opts.clone)?;
    let main = seat.main.clone();
    let (branch, scope) = lease_branch(&seat)?;
    let spec = crate::ship::resolve_spec(opts.spec.as_deref(), &branch, Some(&scope)).ok_or_else(
        || {
            anyhow::anyhow!(
                "could not resolve a spec from the seat's branch `{branch}` — pass one \
                 explicitly: `aida ship <SPEC> --from-seat {}`",
                seat.root.display()
            )
        },
    )?;
    eprintln!(
        "{} aida ship --from-seat — {} on branch {} from {}{}",
        "→".cyan().bold(),
        aida_core::node_alias::display_spec_id(&spec).bold(),
        branch,
        seat.root.display(),
        if opts.dry_run { " (dry run)" } else { "" }
    );
    for line in crate::seat_clone::config_hooks_report(&main, &seat.root) {
        eprintln!("  seat {line}");
    }

    let bytes = (deps.bundle)(&main, &seat.root, &branch)?;
    let staged = stage_bundle(&main, &branch, &bytes, opts.dry_run)?;
    let (quarantine, tip) = (&staged.reference, &staged.tip);
    eprintln!(
        "  {} bundle verified ({tip}){}",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        if opts.dry_run {
            " into a scratch repository"
        } else {
            ""
        }
    );
    let base = crate::detect_default_branch_ref(&main).unwrap_or_else(|| "origin/main".into());
    let base_oid = git_text(
        &main,
        &["rev-parse", "--verify", &format!("{base}^{{commit}}")],
    )?;
    let stat = git_text(
        &staged.repo,
        &["diff", "--stat", &format!("{base_oid}...{quarantine}")],
    )?;
    eprintln!("  changes against {base}:");
    for line in stat.lines() {
        eprintln!("    {line}");
    }
    let store = crate::load_store_for_lookup(&main);
    if store.is_none() {
        eprintln!(
            "{} ship --from-seat: no requirement store reachable; trailer spec-IDs are not \
             resolved",
            "warning:".yellow().bold()
        );
    }
    seat_trailer_check(&staged.repo, &format!("{base_oid}..{quarantine}"), |id| {
        crate::trace::resolve_spec_in_store(store.as_ref(), id)
    })?;
    if opts.dry_run {
        eprintln!(
            "  dry run: would push {tip} to origin/{branch}{}; nothing was imported",
            if opts.no_pr { "" } else { " and open the PR" }
        );
        return Ok(());
    }

    eprintln!("  pushing {tip} to origin/{branch}");
    aida_core::git_ops::clear_sandbox_config_lock(&main);
    let push = aida_core::git_ops::host_git_command(&main)
        .args([
            "push",
            "origin",
            &format!("{quarantine}:refs/heads/{branch}"),
        ])
        .remote_credentials(&main, "origin")
        .status()
        .context("could not invoke `git push`")?;
    if !push.success() {
        anyhow::bail!(
            "`git push origin {quarantine}:refs/heads/{branch}` failed — investigate before \
             retrying"
        );
    }
    // Pushed: the quarantine ref has done its job (a failure above keeps it).
    git_text(&main, &["update-ref", "-d", quarantine, tip])?;
    if opts.no_pr {
        eprintln!(
            "{} aida ship --from-seat — {branch} pushed (--no-pr; no PR opened)",
            crate::glyph(crate::glyphs::Glyph::Check).green().bold()
        );
        return Ok(());
    }

    let forge = (deps.forge)(&main);
    let pr = match forge.change_for_branch(&branch) {
        Ok(crate::forge::ChangeLookup::Found(change)) => {
            eprintln!("  found open PR-{} for branch {branch}", change.id);
            change.id
        }
        _ => open_seat_pr(&*forge, &main, &seat, &branch, tip)?,
    };
    (deps.after_open)(&main, &branch);
    eprintln!(
        "{} aida ship --from-seat — PR-{pr} open for {} (not merged)",
        "⏸".yellow().bold(),
        aida_core::node_alias::display_spec_id(&spec).bold()
    );
    Ok(())
}

/// The PR body for a published seat commit: the commit body, then the note
/// that it was published from the seat by the host.
// trace:BUG-HOME-038 | ai:claude
pub(crate) fn seat_pr_body(message: &str, seat_name: &str) -> String {
    let mut body = crate::pr_ship::derive_pr_body_from_commit(message);
    if !body.trim().is_empty() {
        body.push_str("\n\n");
    }
    body.push_str(&format!(
        "Published from seat `{seat_name}` with `aida ship --from-seat`: bundled inside the \
         seat sandbox, verified and pushed by the host; no seat hook ran."
    ));
    body
}

/// Open the PR for a published seat branch at `tip`: titled with the commit
/// subject, its body [`seat_pr_body`].
// trace:BUG-HOME-038 | ai:claude
fn open_seat_pr(
    forge: &dyn crate::forge::Forge,
    main: &Path,
    seat: &HostRecordedClone,
    branch: &str,
    tip: &str,
) -> Result<u64> {
    let message = git_text(main, &["log", "-1", "--format=%B", tip])?;
    let title = git_text(main, &["log", "-1", "--format=%s", tip])?;
    if title.is_empty() {
        anyhow::bail!("the seat's commit has an empty subject; cannot title the PR");
    }
    let seat_name = seat
        .root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let specs = crate::extract_spec_ids_from_commit(&message);
    let req = crate::forge::OpenChange {
        branch: branch.to_string(),
        base: crate::forge::default_branch_of(main),
        title,
        body: seat_pr_body(&message, &seat_name),
        draft: false,
    };
    let change = crate::gate::ledger::open_change_bound(forge, main, req, &specs)
        .context("could not open a forge change request")?;
    if change.id == 0 {
        anyhow::bail!(
            "the PR was created but its number could not be read: {}",
            change.url
        );
    }
    eprintln!(
        "  {} created PR-{}",
        crate::glyph(crate::glyphs::Glyph::Check).green(),
        change.id
    );
    Ok(change.id)
}
