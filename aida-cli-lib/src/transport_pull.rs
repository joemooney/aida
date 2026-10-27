//! One pinned, fast-forward-only store pull. Incoming history lands in a
//! private staging ref, is scanned like an outbound push, and only then moves
//! the remote-tracking ref and the attached store.
// trace:STORY-1742 | ai:claude

use super::*;

const JOURNAL_FILE: &str = "aida-transport-pull.json";

/// Written before the attached store's index/worktree/ref transition and
/// removed after it. A leftover journal means the transition was interrupted.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u32,
    from: String,
    to: String,
}

fn journal(private: &private::Private) -> Result<Option<Journal>> {
    let Some(bytes) = private.read(JOURNAL_FILE)? else {
        return Ok(None);
    };
    let journal: Journal = serde_json::from_slice(&bytes)
        .ok()
        .filter(|j: &Journal| {
            j.version == 1 && is_hex_id(&j.from, &[40, 64]) && is_hex_id(&j.to, &[j.from.len()])
        })
        .context("Invalid transport pull journal; inspect the store before removing it.")?;
    Ok(Some(journal))
}

// trace:STORY-1742 | ai:claude
pub(super) fn refuse_unfinished(private: &private::Private) -> Result<()> {
    if journal(private)?.is_some() {
        bail!("A previous transport pull did not finish; run `aida pull` to recover it before pushing.");
    }
    Ok(())
}

/// An interrupted transition is recoverable only when the store is cleanly at
/// one of the two journaled commits. Anything else is unknown and refuses.
fn recover(private: &private::Private, store: &Path) -> Result<()> {
    let Some(journal) = journal(private)? else {
        return Ok(());
    };
    if [&journal.to, &journal.from]
        .iter()
        .any(|tip| store_preflight(store, tip).is_ok())
    {
        return private.remove(JOURNAL_FILE);
    }
    bail!(
        "A previous transport pull stopped between {} and {}; the store matches neither cleanly. Inspect it, restore it to one of those commits, then rerun `aida pull`.",
        journal.from,
        journal.to
    );
}

/// The private staging ref is removed on every exit, success or refusal.
struct Staging<'a> {
    store: &'a Path,
}

impl<'a> Staging<'a> {
    fn new(store: &'a Path) -> Result<Self> {
        let staging = Self { store };
        staging.clear();
        if commit(store, INCOMING_REF)?.is_some() {
            bail!("Private transport staging ref could not be cleared; pull refused.");
        }
        Ok(staging)
    }

    fn clear(&self) {
        let _ = aida_core::git_ops::host_git_command(self.store)
            .args(["update-ref", "-d", INCOMING_REF])
            .output();
    }
}

impl Drop for Staging<'_> {
    fn drop(&mut self) {
        self.clear();
    }
}

fn is_ancestor(store: &Path, ancestor: &str, descendant: &str) -> Result<bool> {
    let output = git(
        store,
        &["merge-base", "--is-ancestor", ancestor, descendant],
    )?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => bail!("Cannot verify store ancestry; pull refused. No accepted ref moved."),
    }
}

fn run(store: &Path, args: &[&str]) -> Result<()> {
    let status = aida_core::git_ops::host_git_command(store)
        .args(aida_core::git_ops::NO_AUTO_MAINTENANCE)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if !status.success() {
        bail!("git step failed");
    }
    Ok(())
}

// trace:STORY-1742 | ai:claude
pub(in super::super) fn pull(root: &Path, profile: &Profile) -> Result<()> {
    let common = common_dir(root)?.context("Transport pull requires a Git clone")?;
    let _configuration_lock = configuration_lock(&common)?;
    let private = private::Private::open(&common)?;
    if load(&common)?.as_ref() != Some(profile) {
        bail!("Transport profile changed during planning; pull refused.");
    }
    refuse_partial_clone(root)?;
    let endpoint = pinned_endpoint(root, profile)?;
    let store = attached_store(root, &common)?;
    // Missing policy or trust root refuses before any network contact.
    let scan = scan::Scan::load(&private)?;
    if profile.baseline.is_none() {
        bail!("Explicit reviewed trust root is missing; pull refused.");
    }
    refuse_endpoint_rewrites(&store, &endpoint)?;
    let busy = |_| {
        anyhow::anyhow!("Store writer lock or verified hook profile unavailable; pull refused. Check for a busy writer, lock permissions, or unsupported hooks before retrying.")
    };
    let local = aida_core::db::with_store_write_lock(&store, || {
        Ok((|| {
            recover(&private, &store)?;
            let local = commit(&store, STORE_REF)?
                .context("Local aida-store tip is missing; pull refused.")?;
            store_preflight(&store, &local).map(|()| local)
        })())
    })
    .map_err(busy)??;
    let tracking = commit(&store, TRACKING_REF)?;

    // Network I/O happens with no store writer lock held.
    if aida_core::db::any_store_write_lock_held() {
        bail!("Transport cannot pull while a store writer lock is held.");
    }
    let staging = Staging::new(&store)?;
    let (destination, credentials) = aida_core::git_ops::split_http_url_userinfo(&endpoint);
    if credentials.is_some() && endpoint.trim() != endpoint {
        bail!("Ambiguous credentialed origin endpoint; pull refused.");
    }
    refuse_endpoint_rewrites(&store, &destination)?;
    // The explicit endpoint and refspec bypass remote.origin.* fetch config,
    // so no configured refmap, tag following or prune can move other refs.
    // Objects arrive locally; only the private staging ref names them.
    let refspec = format!("{STORE_REF}:{INCOMING_REF}");
    let mut fetch = aida_core::git_ops::host_git_command(&store);
    fetch
        .args(aida_core::git_ops::NO_AUTO_MAINTENANCE)
        .args([
            "-c",
            "fetch.fsckObjects=true",
            "-c",
            "fetch.writeCommitGraph=false",
        ])
        .env("GIT_NO_LAZY_FETCH", "1")
        .args([
            "fetch",
            "--no-tags",
            "--no-write-fetch-head",
            "--recurse-submodules=no",
            "--quiet",
            "--end-of-options",
            &destination,
            &refspec,
        ]);
    crate::gitlab_project_create::configure_git_remote_credentials(
        &mut fetch,
        &store,
        &endpoint,
        Some(&endpoint),
    );
    let (status, _, interrupted) = scan::publish(fetch, 65536).map_err(|_| {
        anyhow::anyhow!("Store fetch execution is uncertain; pull refused. No accepted ref moved.")
    })?;
    if interrupted {
        bail!("Store fetch was interrupted; pull refused. No accepted ref moved.");
    }
    if !status.success() {
        // Remote diagnostics can contain emails, URLs or hostnames.
        bail!("Store fetch failed or was refused by origin; pull refused. No accepted ref moved. Terminal prompts are disabled: configure a credential helper or SSH agent (and known host key) for origin.");
    }
    let incoming = commit(&store, INCOMING_REF)?
        .context("Fetched store tip is unavailable; pull refused. No accepted ref moved.")?;
    if incoming.len() != local.len() {
        bail!("Fetched store tip uses a different object format; pull refused. No accepted ref moved.");
    }
    if pinned_endpoint(root, profile)? != endpoint {
        bail!("Origin changed during store fetch; pull refused. No accepted ref moved.");
    }
    scan.revalidate(&private)?;
    if load(&common)?.as_ref() != Some(profile) {
        bail!("Transport profile changed; pull refused. No accepted ref moved.");
    }

    let outcome = aida_core::db::with_store_write_lock(&store, || {
        Ok((|| {
            store_preflight(&store, &local)?;
            if commit(&store, TRACKING_REF)? != tracking {
                bail!("Remote-tracking aida-store moved during pull; pull refused. No accepted ref moved.");
            }
            let forward = is_ancestor(&store, &local, &incoming)?;
            if !forward && !is_ancestor(&store, &incoming, &local)? {
                bail!("Local and origin aida-store have diverged; transport pull is fast-forward only. No accepted ref moved; reconcile explicitly outside the transport profile.");
            }
            // Incoming history is never a new trust root: scan from the
            // enrolled root exactly as a push of that tip would be scanned.
            scan.history(&store, profile, &incoming)?;
            scan.revalidate(&private)?;
            transition(&private, &store, &local, &incoming, forward, tracking.as_deref())
        })())
    })
    .map_err(busy)??;
    drop(staging);

    match outcome.as_slice() {
        [] => println!(
            "aida-store is already up to date with pinned origin ({incoming}); no ref moved."
        ),
        moved => {
            println!(
                "Pulled aida-store {incoming} from pinned origin (store ref only, fast-forward)."
            );
            println!("Moved: {}", moved.join(", "));
            if !moved.contains(&STORE_REF) {
                println!("Local aida-store already contains origin's tip; only the remote-tracking ref moved.");
            }
        }
    }
    println!(
        "Incoming history scan passed; no commit, mailbox, ID, reconciliation or fan-out step ran."
    );
    Ok(())
}

/// Remote-tracking first (it only records what origin holds, now scanned),
/// then the attached store's index/worktree, then its branch ref, each with
/// an old-value check. A failure reports exactly which steps completed.
fn transition(
    private: &private::Private,
    store: &Path,
    local: &str,
    incoming: &str,
    forward: bool,
    tracking: Option<&str>,
) -> Result<Vec<&'static str>> {
    let mut moved = Vec::new();
    if tracking != Some(incoming) {
        let zero = "0".repeat(incoming.len());
        let old = tracking.unwrap_or(&zero);
        run(
            store,
            &[
                "update-ref",
                "-m",
                "aida transport pull",
                TRACKING_REF,
                incoming,
                old,
            ],
        )
        .map_err(|_| {
            anyhow::anyhow!("Remote-tracking aida-store could not be advanced; pull refused. No accepted ref moved.")
        })?;
        moved.push(TRACKING_REF);
    }
    if !forward || local == incoming {
        return Ok(moved);
    }
    let partial = if moved.is_empty() {
        ""
    } else {
        "refs/remotes/origin/aida-store already moved. "
    };
    // trace:STORY-1742 | ai:claude — a journal failure still names the partial step.
    let journal = serde_json::to_vec(&Journal {
        version: 1,
        from: local.into(),
        to: incoming.into(),
    })?;
    private.write(JOURNAL_FILE, &journal).map_err(|_| {
        anyhow::anyhow!("The private recovery journal could not be written; {partial}refs/heads/aida-store still names {local} and the store index/worktree were not touched. Pull refused.")
    })?;
    run(store, &["read-tree", "-m", "-u", local, incoming]).map_err(|_| {
        anyhow::anyhow!("Store index/worktree transition from {local} to {incoming} did not complete; {partial}refs/heads/aida-store still names {local}. The recovery journal is kept: inspect the store, restore it cleanly to either commit, then rerun `aida pull`.")
    })?;
    run(
        store,
        &[
            "update-ref",
            "-m",
            "aida transport pull (fast-forward)",
            STORE_REF,
            incoming,
            local,
        ],
    )
    .map_err(|_| {
        anyhow::anyhow!("Store index/worktree now match {incoming}, but refs/heads/aida-store could not be advanced from {local}; {partial}The recovery journal is kept: inspect the store, restore it cleanly to either commit, then rerun `aida pull`.")
    })?;
    moved.push(STORE_REF);
    private.remove(JOURNAL_FILE).map_err(|_| {
        anyhow::anyhow!("aida-store advanced to {incoming}, but the recovery journal could not be cleared; rerun `aida pull` to confirm the store state.")
    })?;
    Ok(moved)
}
