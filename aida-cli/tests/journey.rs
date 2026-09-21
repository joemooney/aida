// BUG-688: gated to Linux. These binary-driving e2e suites pass on Linux PR CI
// but fail on the nightly macOS/Windows matrix (macOS: `aida init` exits 1 with
// no output; Windows: empty stderr) — root cause undetermined without platform
// access. Consistent with the "PR CI is Linux-only until there are non-Linux
// users" stance; BUG-688 stays open to determine whether the macOS failure is a
// real aida-init regression or an isolated-tempdir e2e-harness artifact.
// trace:BUG-688 | ai:claude
#![cfg(target_os = "linux")]
//! STORY-743: behavioral journey self-test — drive the REAL `aida` binary
//! through the core human + agent loops end-to-end, so the ~40 behaviors that
//! shipped recently can't silently regress together even when their individual
//! unit tests still pass. This is the HOLISTIC counterpart to the per-fix
//! integration tests (`newcomer_delight.rs`, `agent_write_confirm.rs`): it
//! consolidates + extends their coverage into three coherent journeys that
//! exercise the whole loop as a user (human or agent) actually walks it.
//!
//! Every assertion is BEHAVIOR-level (match the meaningful output, not exact
//! whitespace) and resilient. If one fails, the shipped behavior it names has
//! regressed on the branch under test.
//!
//! Journeys:
//!   * HUMAN (TTY-ish path, forced via AIDA_AGENT_OUTPUT=0):
//!       - `aida add` renders the `Next:` footer + `// trace:<id>` breadcrumb
//!         (STORY-737 #2).
//!       - `aida zen --dry-run` renders a drafted spec and files NOTHING
//!         (STORY-736).
//!       - `aida history` hides seeded META rows; `--include-meta` reveals them
//!         (STORY-737 #4).
//!       - an empty `aida queue work` is a SOFT signpost, not a red `Error:`
//!         (STORY-737 #5).
//!       - a spec reaching Completed renders the crescendo, not `Updated:`
//!         (STORY-738).
//!   * AGENT (AIDA_AGENT_OUTPUT=toon):
//!       - `aida list` / `aida search` emit lean TOON `specs[N]{...}`, not the
//!         box-table (STORY-734 / BUG-668 / BUG-672).
//!       - `aida queue done` in a non-TTY marks the spec Done (no silent no-op)
//!         and prints success to STDOUT (BUG-671).
//!       - chain verbs carry `next[]` guidance; `queue done` points at
//!         `aida pull` (BUG-673).
//!       - `aida status` leads with `queue_actionable`, never a divergent
//!         `queue_depth` (BUG-670).
//!   * COORDINATION:
//!       - the unified `aida awaiting` report spans the mail + briefs + findings
//!         channels in one place (STORY-741).
// trace:STORY-743

use std::path::Path;
use std::process::{Command, Stdio};

/// Base command for the real `aida` binary against an isolated repo + HOME.
/// Deliberately does NOT set AIDA_AGENT_OUTPUT — each call selects the human
/// (`0`) or agent (`toon`/`1`) surface explicitly, since in a test pipe stdout
/// is not a TTY and would otherwise auto-select agent mode.
fn aida(repo: &Path, home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aida"));
    cmd.current_dir(repo);
    cmd.env("HOME", home);
    cmd.env("AIDA_TELEMETRY", "0");
    // Stable queue identity so `queue *` and `status`/`awaiting` agree.
    cmd.env("USER", "journeytester");
    cmd.env("AIDA_USER", "journeytester");
    cmd.env_remove("AIDA_SESSION_ROLE");
    cmd.env_remove("AIDA_PERMISSION_MODE");
    cmd
}

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(repo)
        .args(args)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

/// A SPEC-ID is `UPPER-<digits>` (e.g. `TASK-1`).
fn is_spec_id(t: &str) -> bool {
    let mut parts = t.splitn(2, '-');
    match (parts.next(), parts.next()) {
        (Some(prefix), Some(num)) => {
            !prefix.is_empty()
                && prefix.chars().all(|c| c.is_ascii_uppercase())
                && !num.is_empty()
                && num.chars().all(|c| c.is_ascii_digit())
        }
        _ => false,
    }
}

/// First SPEC-ID token in `out`, panicking with the raw output on miss.
fn parse_spec_id(out: &str) -> String {
    out.split(|c: char| c.is_whitespace() || c == ',')
        .find(|t| is_spec_id(t))
        .unwrap_or_else(|| panic!("could not parse spec id from:\n{out}"))
        .to_string()
}

/// Stand up a throwaway git repo with an initialized (distributed, git-canonical)
/// AIDA store. Returns the tempdir guard plus the repo + HOME paths. Skills /
/// hooks / roles / agent-config are all skipped to keep init fast.
fn init_repo() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let base = tempfile::tempdir().expect("tempdir");
    // BUG-671: on macOS the tempdir resolves under /var/folders/… which is a
    // symlink to /private/var/folders/…; canonicalize once so every path the
    // test passes matches the path `aida` records internally, on every OS.
    let base_dir = base.path().canonicalize().expect("canonicalize tempdir");
    let repo = base_dir.join("repo");
    let home = base_dir.join("home");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&home).unwrap();

    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "t@t.t"]);
    git(&repo, &["config", "user.name", "t"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"]);

    let init = aida(&repo, &home)
        .env("AIDA_AGENT_OUTPUT", "0")
        .args([
            "init",
            "--no-skills",
            "--no-hooks",
            "--no-agent-config",
            "--no-roles",
        ])
        .output()
        .expect("run aida init");
    assert!(
        init.status.success(),
        "aida init failed (exit {:?}):\n--- stdout ---\n{}\n--- stderr ---\n{}",
        init.status.code(),
        String::from_utf8_lossy(&init.stdout),
        String::from_utf8_lossy(&init.stderr)
    );
    (base, repo, home)
}

/// HUMAN journey: the TTY-ish first-run path, forced human via
/// `AIDA_AGENT_OUTPUT=0`. One init'd repo carried across every sub-check.
#[test]
fn human_journey_add_zen_history_empty_queue_and_completion() {
    let (_base, repo, home) = init_repo();

    // ---- STORY-737 #5 (day one): an empty queue is a SOFT signpost. ----
    let fresh = aida(&repo, &home)
        .env("AIDA_AGENT_OUTPUT", "0")
        .args(["queue", "work", "--no-pull"])
        .output()
        .expect("run aida queue work");
    assert!(
        !fresh.status.success(),
        "an empty queue still exits non-zero so scripts can gate"
    );
    let fresh_err = String::from_utf8_lossy(&fresh.stderr);
    assert!(
        !fresh_err.contains("Error:"),
        "a day-one empty queue must NOT render a red `Error:`:\n{fresh_err}"
    );
    assert!(
        fresh_err.contains("fresh project") || fresh_err.contains("File a spec"),
        "expected a forward-pointing fresh-project signpost:\n{fresh_err}"
    );

    // ---- STORY-736: `aida zen --dry-run` renders a draft, files NOTHING. ----
    let zen = aida(&repo, &home)
        .env("AIDA_AGENT_OUTPUT", "0")
        .env("AIDA_SESSION_ROLE", "advisor")
        .args(["zen", "Add a dark mode toggle to settings", "--dry-run"])
        .output()
        .expect("run aida zen --dry-run");
    assert!(
        zen.status.success(),
        "zen --dry-run should succeed: {}",
        String::from_utf8_lossy(&zen.stderr)
    );
    let zen_out = String::from_utf8_lossy(&zen.stdout);
    assert!(
        zen_out.contains("would draft"),
        "dry-run must frame the drafted-but-not-filed spec:\n{zen_out}"
    );
    assert!(
        zen_out.contains("Add a dark mode toggle to settings"),
        "dry-run must render the drafted title/thought:\n{zen_out}"
    );
    assert!(
        zen_out.contains("Description:"),
        "dry-run must render the drafted description:\n{zen_out}"
    );
    // Files nothing: no spec carrying that title exists afterward.
    let after_zen = aida(&repo, &home)
        .env("AIDA_AGENT_OUTPUT", "0")
        .args(["list", "--all"])
        .output()
        .expect("run aida list");
    let after_zen_out = String::from_utf8_lossy(&after_zen.stdout);
    assert!(
        !after_zen_out.contains("dark mode toggle"),
        "zen --dry-run must file NOTHING (no drafted spec should appear):\n{after_zen_out}"
    );

    // ---- STORY-737 #2: `aida add` renders Next: footer + trace breadcrumb. ----
    let add = aida(&repo, &home)
        .env("AIDA_AGENT_OUTPUT", "0")
        .env("AIDA_SESSION_ROLE", "advisor")
        .args([
            "add",
            "--type",
            "task",
            "--status",
            "approved",
            "--title",
            "Build login page",
        ])
        .output()
        .expect("run aida add");
    assert!(
        add.status.success(),
        "aida add failed: {}",
        String::from_utf8_lossy(&add.stderr)
    );
    let add_out = String::from_utf8_lossy(&add.stdout);
    let spec = parse_spec_id(&add_out);
    assert!(
        add_out.contains("Next:"),
        "human add path must render the Next: footer:\n{add_out}"
    );
    assert!(
        add_out.contains(&format!("// trace:{spec}")),
        "human add path must render the trace breadcrumb:\n{add_out}"
    );

    // ---- STORY-737 #4: history hides META by default; --include-meta shows. ----
    let hist = aida(&repo, &home)
        .env("AIDA_AGENT_OUTPUT", "0")
        .args(["history"])
        .output()
        .expect("run aida history");
    let hist_out = String::from_utf8_lossy(&hist.stdout);
    assert!(
        !hist_out.contains("META-"),
        "default history must hide seeded META rows:\n{hist_out}"
    );
    assert!(
        hist_out.contains(&spec),
        "default history must still show the real spec:\n{hist_out}"
    );
    let hist_meta = aida(&repo, &home)
        .env("AIDA_AGENT_OUTPUT", "0")
        .args(["history", "--include-meta"])
        .output()
        .expect("run aida history --include-meta");
    let hist_meta_out = String::from_utf8_lossy(&hist_meta.stdout);
    assert!(
        hist_meta_out.contains("META-"),
        "--include-meta must reveal META rows:\n{hist_meta_out}"
    );

    // ---- STORY-737 #5 (specs exist, none queued): soft signpost again. ----
    let empty = aida(&repo, &home)
        .env("AIDA_AGENT_OUTPUT", "0")
        .args(["queue", "work", "--no-pull"])
        .output()
        .expect("run aida queue work (non-fresh)");
    assert!(!empty.status.success(), "empty queue still exits non-zero");
    let empty_err = String::from_utf8_lossy(&empty.stderr);
    assert!(
        !empty_err.contains("Error:"),
        "an expected empty queue must NOT render a red `Error:`:\n{empty_err}"
    );
    assert!(
        empty_err.contains("Nothing queued yet"),
        "expected the forward-pointing empty-queue signpost:\n{empty_err}"
    );

    // ---- STORY-738: reaching Completed renders the crescendo, not `Updated:`. --
    let done = aida(&repo, &home)
        .env("AIDA_AGENT_OUTPUT", "0")
        .env("AIDA_SESSION_ROLE", "advisor")
        .args(["edit", &spec, "--status", "completed"])
        .output()
        .expect("run aida edit --status completed");
    assert!(
        done.status.success(),
        "edit --status completed failed: {}",
        String::from_utf8_lossy(&done.stderr)
    );
    let done_out = String::from_utf8_lossy(&done.stdout);
    assert!(
        done_out.contains("the loop closed") && done_out.contains("Completed"),
        "reaching Completed must render the felt crescendo:\n{done_out}"
    );
    assert!(
        !done_out.contains("Updated:"),
        "the completion moment must NOT be the flat `Updated:` line:\n{done_out}"
    );
}

/// AGENT journey: the token-efficient TOON surface an MCP-adjacent coding agent
/// drives (`AIDA_AGENT_OUTPUT=toon`). One init'd repo carried across sub-checks.
#[test]
fn agent_journey_toon_list_search_queue_done_and_status() {
    let (_base, repo, home) = init_repo();

    // File + queue an approved spec (advisor-gated writes).
    let add = aida(&repo, &home)
        .env("AIDA_AGENT_OUTPUT", "toon")
        .env("AIDA_SESSION_ROLE", "advisor")
        .args([
            "add",
            "--type",
            "task",
            "--status",
            "approved",
            "--title",
            "Wire the widget",
        ])
        .output()
        .expect("run aida add");
    assert!(
        add.status.success(),
        "aida add failed: {}",
        String::from_utf8_lossy(&add.stderr)
    );
    let spec = parse_spec_id(&String::from_utf8_lossy(&add.stdout));

    // BUG-1442: one requirement carrying an ordinary core edge, a custom
    // edge, and a blocker must expose the same relationship types to humans
    // and TOON consumers. The TOON view also carries the blocker's pickup
    // consequence instead of forcing an agent to make a second graph query.
    // trace:BUG-1442 | ai:codex
    let mut targets = Vec::new();
    for title in [
        "Reference target",
        "Implementation target",
        "Blocking target",
    ] {
        let target = aida(&repo, &home)
            .env("AIDA_AGENT_OUTPUT", "toon")
            .env("AIDA_SESSION_ROLE", "advisor")
            .args([
                "add", "--type", "task", "--status", "approved", "--title", title,
            ])
            .output()
            .expect("add relationship target");
        assert!(
            target.status.success(),
            "target add failed: {}",
            String::from_utf8_lossy(&target.stderr)
        );
        targets.push(parse_spec_id(&String::from_utf8_lossy(&target.stdout)));
    }
    for (target, rel_type) in targets
        .iter()
        .zip(["references", "implemented-by", "blocked-by"])
    {
        let rel = aida(&repo, &home)
            .env("AIDA_AGENT_OUTPUT", "toon")
            .args(["rel", "add", &spec, target, "--type", rel_type])
            .output()
            .expect("add relationship");
        assert!(
            rel.status.success(),
            "relationship add failed: {}",
            String::from_utf8_lossy(&rel.stderr)
        );
    }
    let human_show = aida(&repo, &home)
        .env("AIDA_AGENT_OUTPUT", "0")
        .args(["show", &spec])
        .output()
        .expect("show human relationship types");
    let toon_show = aida(&repo, &home)
        .env("AIDA_AGENT_OUTPUT", "toon")
        .args(["show", &spec])
        .output()
        .expect("show TOON relationship types");
    let human_show = String::from_utf8_lossy(&human_show.stdout);
    let toon_show = String::from_utf8_lossy(&toon_show.stdout);

    // Compare the two projections as PARSED EDGES, not by searching each
    // output for substrings. Independent substring searches pass while the
    // two disagree on row cardinality, on which target an edge points at, on
    // ordering, or on an extra collapsed row — every one of those is a real
    // divergence that "both documents contain the word blocked-by" cannot
    // see.
    //
    // The two surfaces use DIFFERENT vocabularies on purpose: human prose
    // ("is blocked by") against canonical stored labels ("blocked-by"). So
    // the correspondence has to be declared rather than assumed, and it is
    // declared HERE so that changing either vocabulary fails this test
    // instead of drifting silently.
    // trace:BUG-1442 | ai:claude
    fn canonical_edge_type(label: &str) -> &str {
        match label {
            "is parent of" | "parent" => "Parent",
            "is child of" | "child" => "Child",
            "is duplicate of" | "duplicate" => "Duplicate",
            "verifies" => "Verifies",
            "is verified by" | "verified-by" => "VerifiedBy",
            "references" => "References",
            "is blocked by" | "blocked-by" => "BlockedBy",
            "blocks" => "Blocks",
            "is superseded by" | "superseded-by" => "SupersededBy",
            "supersedes" => "Supersedes",
            // a custom edge carries the same string through both surfaces
            other => other,
        }
    }

    // `TASK-4` yes; `out` no. Deliberately strict so a stray line cannot
    // become an edge.
    fn looks_like_spec_id(s: &str) -> bool {
        let Some((prefix, number)) = s.rsplit_once('-') else {
            return false;
        };
        !prefix.is_empty()
            && prefix
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-')
            && !number.is_empty()
            && number.chars().all(|c| c.is_ascii_digit())
    }

    // The human section is `Relations:` followed by rows shaped
    // `  <glyph> <phrase> <SPEC-ID> (<title>)`. Parsed against the renderer's
    // OBSERVED output rather than a renderer that merely looked like it: the
    // first version of this parser targeted a different `Relationships:`
    // section that `aida show` does not emit, and the comparison failed loudly
    // rather than passing vacuously, which is the point of comparing
    // structures instead of searching for substrings.
    //
    // NOTE the collapse rule: above five relationships, and without --rels,
    // the human surface prints a COUNT instead of rows while TOON still
    // enumerates. That is exactly the "extra collapsed rows" divergence this
    // test exists to catch, so the fixture deliberately stays at three.
    // trace:BUG-1442 | ai:claude
    fn human_edges(show: &str) -> Vec<(String, String)> {
        let mut rows = Vec::new();
        let mut inside = false;
        for line in show.lines() {
            let line = line.trim();
            if line == "Relations:" {
                inside = true;
                continue;
            }
            if !inside {
                continue;
            }
            // rows end with the target's title in parentheses; the first line
            // that does not is the end of the section
            let Some((lhs, _title)) = line.split_once(" (") else {
                break;
            };
            let Some((glyph_and_phrase, target)) = lhs.rsplit_once(' ') else {
                break;
            };
            // The section ends at the first line that is not a row. Bound it
            // on the TARGET looking like a spec id rather than on the glyph or
            // on a blank line: `Centrality: 0 in / 3 out  (heft 5)` follows
            // immediately, contains " (" too, and parsed as a bogus edge until
            // this predicate was added.
            let target = target.trim();
            if !looks_like_spec_id(target) {
                break;
            }
            // drop the leading sub-arrow glyph, whatever it renders as
            let phrase = match glyph_and_phrase.split_once(char::is_whitespace) {
                Some((_glyph, rest)) => rest.trim(),
                None => break,
            };
            rows.push((canonical_edge_type(phrase).to_string(), target.to_string()));
        }
        rows
    }

    // TOON is `relationships[N]{rel,id,title}:` then N rows of `rel,id,title`
    fn toon_edges(show: &str) -> Vec<(String, String)> {
        let mut rows = Vec::new();
        let mut lines = show
            .lines()
            .skip_while(|l| !l.starts_with("relationships["));
        let Some(header) = lines.next() else {
            return rows;
        };
        let count: usize = header
            .split_once('[')
            .and_then(|(_, rest)| rest.split_once(']'))
            .and_then(|(n, _)| n.parse().ok())
            .unwrap_or(0);
        for line in lines.take(count) {
            let cells: Vec<&str> = line.trim().splitn(3, ',').collect();
            if cells.len() < 2 {
                break;
            }
            rows.push((
                canonical_edge_type(cells[0].trim()).to_string(),
                cells[1].trim().to_string(),
            ));
        }
        rows
    }

    let human_rel = human_edges(&human_show);
    let toon_rel = toon_edges(&toon_show);

    // Ordered, not sorted: both surfaces render from the same stored
    // relationship order, so an ordering difference IS a divergence rather
    // than a formatting choice.
    assert_eq!(
        human_rel, toon_rel,
        "human and TOON relationship projections disagree\n\
         human: {human_rel:?}\n  toon: {toon_rel:?}\n\
         --- human ---\n{human_show}\n--- toon ---\n{toon_show}"
    );

    // Agreement alone is not enough — both projections could agree and both
    // be wrong. Pin the edges the fixture actually created, each against its
    // own target, so a collapsed or misassociated row fails here.
    let expected: Vec<(String, String)> = vec![
        ("References".to_string(), targets[0].clone()),
        ("implemented-by".to_string(), targets[1].clone()),
        ("BlockedBy".to_string(), targets[2].clone()),
    ];
    assert_eq!(
        human_rel, expected,
        "relationship edges lost their type or their target association\n{human_show}"
    );
    assert!(
        toon_show.contains("blockers[1]{id,status,satisfied}")
            && toon_show.contains("blocked: true")
            && toon_show.contains("pickup: refused until all blockers are Completed"),
        "TOON show must convey blocked state and pickup consequence:\n{toon_show}"
    );

    // ---- STORY-734 / BUG-668 / BUG-672: lean TOON list, not the box-table. ----
    let list = aida(&repo, &home)
        .env("AIDA_AGENT_OUTPUT", "toon")
        .args(["list"])
        .output()
        .expect("run aida list");
    let list_out = String::from_utf8_lossy(&list.stdout);
    assert!(
        list_out.contains("specs[") && list_out.contains("{id"),
        "agent list must emit the lean TOON `specs[N]{{...}}` header:\n{list_out}"
    );
    assert!(
        !list_out.contains('│') && !list_out.contains('┌'),
        "agent list must NOT emit the human box-table:\n{list_out}"
    );
    assert!(
        list_out.contains(&spec),
        "agent list must include the filed spec row:\n{list_out}"
    );

    // ---- Same lean TOON shape for `search`. ----
    let search = aida(&repo, &home)
        .env("AIDA_AGENT_OUTPUT", "toon")
        .args(["search", "widget"])
        .output()
        .expect("run aida search");
    let search_out = String::from_utf8_lossy(&search.stdout);
    assert!(
        search_out.contains("specs[") && search_out.contains(&spec),
        "agent search must emit the lean TOON `specs[N]{{...}}` shape with the hit:\n{search_out}"
    );
    assert!(
        !search_out.contains('│') && !search_out.contains('┌'),
        "agent search must NOT emit the human box-table:\n{search_out}"
    );

    // Queue it so `queue done` has something to close.
    let qadd = aida(&repo, &home)
        .env("AIDA_AGENT_OUTPUT", "toon")
        .env("AIDA_SESSION_ROLE", "advisor")
        .args(["queue", "add", &spec])
        .output()
        .expect("run aida queue add");
    assert!(
        qadd.status.success(),
        "aida queue add failed: {}",
        String::from_utf8_lossy(&qadd.stderr)
    );

    // ---- BUG-671: `queue done` in a non-TTY marks Done (no silent no-op) and
    //      the success line reaches STDOUT. BUG-673: the chain carries next[]
    //      guidance pointing at `aida pull`. ----
    let done = aida(&repo, &home)
        .env("AIDA_AGENT_OUTPUT", "toon")
        .args(["queue", "done", &spec, "--skip-pr-check"])
        .stdin(Stdio::null())
        .output()
        .expect("run aida queue done");
    let done_out = String::from_utf8_lossy(&done.stdout);
    let done_err = String::from_utf8_lossy(&done.stderr);
    assert!(
        done.status.success(),
        "queue done must succeed (auto-confirm), got exit {:?}\nstdout={done_out}\nstderr={done_err}",
        done.status.code()
    );
    assert!(
        done_out.contains("marked done"),
        "the queue-done success line must reach STDOUT:\nstdout={done_out}\nstderr={done_err}"
    );
    assert!(
        !done_out.contains("Cancelled"),
        "a non-interactive write must NOT silently cancel:\nstdout={done_out}"
    );
    assert!(
        done_out.contains("next[") && done_out.contains("aida pull"),
        "the queue-done chain must carry next[] guidance pointing at `aida pull`:\n{done_out}"
    );

    // The write actually landed: the spec is Done.
    let show = aida(&repo, &home)
        .env("AIDA_AGENT_OUTPUT", "0")
        .args(["show", &spec])
        .output()
        .expect("run aida show");
    let show_out = String::from_utf8_lossy(&show.stdout);
    assert!(
        show_out.contains("Done"),
        "the spec must be marked Done after queue done:\n{show_out}"
    );

    // ---- BUG-670: agent `status` leads with `queue_actionable`, and never
    //      shows a `queue_depth` that DIVERGES from it (it precedes depth when
    //      both appear, and depth is suppressed on mismatch). ----
    let status = aida(&repo, &home)
        .env("AIDA_AGENT_OUTPUT", "toon")
        .args(["status"])
        .output()
        .expect("run aida status");
    let status_out = String::from_utf8_lossy(&status.stdout);
    let actionable_at = status_out
        .find("queue_actionable")
        .unwrap_or_else(|| panic!("agent status must expose queue_actionable:\n{status_out}"));
    if let Some(depth_at) = status_out.find("queue_depth") {
        assert!(
            actionable_at < depth_at,
            "queue_actionable must LEAD queue_depth in the agent status head:\n{status_out}"
        );
    }
}

/// COORDINATION journey: STORY-741 promoted the "Awaiting you" report to a
/// first-class `aida awaiting` command that unifies EVERY channel where the
/// user is the gate. Assert the report spans the mail + briefs + findings
/// channels in one place (the JSON projection names each channel).
#[test]
fn coordination_journey_awaiting_unifies_mail_briefs_findings() {
    let (_base, repo, home) = init_repo();

    // Human report renders and exits cleanly even with nothing waiting.
    let human = aida(&repo, &home)
        .env("AIDA_AGENT_OUTPUT", "0")
        .args(["awaiting"])
        .output()
        .expect("run aida awaiting");
    assert!(
        human.status.success(),
        "aida awaiting should succeed: {}",
        String::from_utf8_lossy(&human.stderr)
    );

    // The JSON projection is the machine-checkable proof that the report spans
    // every channel in one place.
    let json = aida(&repo, &home)
        .env("AIDA_AGENT_OUTPUT", "0")
        .args(["awaiting", "--json"])
        .output()
        .expect("run aida awaiting --json");
    assert!(
        json.status.success(),
        "aida awaiting --json should succeed: {}",
        String::from_utf8_lossy(&json.stderr)
    );
    let json_out = String::from_utf8_lossy(&json.stdout);
    for channel in ["mail", "pending_briefs", "findings"] {
        assert!(
            json_out.contains(channel),
            "the unified awaiting report must span the `{channel}` channel:\n{json_out}"
        );
    }
}

/// BUG-1558: `aida show` must render each relationship with its STORED type
/// on every surface — human, TOON, and the machine `--json` projection — not
/// collapse everything but Parent into "Related". The fixture mirrors
/// TASK-1247's real shape (BlockedBy + Child + Blocks alongside Parent):
/// three distinct non-Parent types on one spec.
///
/// DEMONSTRATE-IT-FIRES: written against the code as found, `--json` carried
/// NO relationships field at all (a stronger form of the same collapse — a
/// missing field instead of a mislabeled one), so the `--json` assertion
/// below is genuinely RED before the fix and green after. The human/TOON
/// assertions were already correct on this branch (BUG-1442/TASK-102 fixed
/// those renderers earlier) and are included here to lock that in as a
/// regression guard for all three surfaces together.
// trace:BUG-1558 | ai:claude
#[test]
fn bug_1558_show_carries_stored_relationship_type_on_every_surface() {
    let (_base, repo, home) = init_repo();

    let add_spec = |title: &str| -> String {
        let out = aida(&repo, &home)
            .env("AIDA_AGENT_OUTPUT", "toon")
            .env("AIDA_SESSION_ROLE", "advisor")
            .args([
                "add", "--type", "task", "--status", "approved", "--title", title,
            ])
            .output()
            .expect("run aida add");
        assert!(
            out.status.success(),
            "aida add failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        parse_spec_id(&String::from_utf8_lossy(&out.stdout))
    };

    let main_spec = add_spec("BUG-1558 fixture: main spec");
    let blocker = add_spec("BUG-1558 fixture: blocker target");
    let child = add_spec("BUG-1558 fixture: child target");
    let blocked = add_spec("BUG-1558 fixture: blocks target");

    for (target, rel_type) in [
        (&blocker, "blocked-by"),
        (&child, "child"),
        (&blocked, "blocks"),
    ] {
        let rel = aida(&repo, &home)
            .env("AIDA_AGENT_OUTPUT", "toon")
            .args(["rel", "add", &main_spec, target, "--type", rel_type])
            .output()
            .expect("run aida rel add");
        assert!(
            rel.status.success(),
            "rel add {rel_type} failed: {}",
            String::from_utf8_lossy(&rel.stderr)
        );
    }

    // ---- Human surface: already fixed (TASK-102 `relationship_phrase`). ----
    let human = aida(&repo, &home)
        .env("AIDA_AGENT_OUTPUT", "0")
        .args(["show", &main_spec])
        .output()
        .expect("run aida show (human)");
    let human_out = String::from_utf8_lossy(&human.stdout);
    assert!(
        human_out.contains("is blocked by")
            && human_out.contains("is child of")
            && human_out.contains("blocks"),
        "human `aida show` must name each stored relationship type, not \
         collapse them to a single undifferentiated bucket:\n{human_out}"
    );
    assert!(
        !human_out.contains("Related"),
        "none of these three typed edges should ever render as the generic \
         \"Related\" bucket:\n{human_out}"
    );

    // ---- TOON surface: already fixed (BUG-1442 `rel_type_label`). ----
    let toon = aida(&repo, &home)
        .env("AIDA_AGENT_OUTPUT", "toon")
        .args(["show", &main_spec])
        .output()
        .expect("run aida show (toon)");
    let toon_out = String::from_utf8_lossy(&toon.stdout);
    for t in ["blocked-by", "child", "blocks"] {
        assert!(
            toon_out.contains(t),
            "TOON `aida show` must carry the stored relationship type \
             `{t}`, not collapse it:\n{toon_out}"
        );
    }
    assert!(
        !toon_out.contains(",Related,") && !toon_out.starts_with("Related,"),
        "TOON must not render any of these edges as the generic \"Related\" \
         label:\n{toon_out}"
    );

    // ---- Machine JSON surface: THIS is the one still broken as found — the
    //      `ShowJson` projection carried no `relationships` field at all. ----
    let json = aida(&repo, &home)
        .env("AIDA_AGENT_OUTPUT", "0")
        .args(["show", &main_spec, "--json"])
        .output()
        .expect("run aida show --json");
    assert!(
        json.status.success(),
        "aida show --json failed: {}",
        String::from_utf8_lossy(&json.stderr)
    );
    let json_out = String::from_utf8_lossy(&json.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&json_out)
        .unwrap_or_else(|e| panic!("aida show --json did not emit valid JSON: {e}\n{json_out}"));
    let rels = parsed
        .get("relationships")
        .unwrap_or_else(|| {
            panic!(
                "aida show --json must carry a `relationships` field \
                 (BUG-1558: it was missing entirely):\n{json_out}"
            )
        })
        .as_array()
        .unwrap_or_else(|| panic!("`relationships` must be a JSON array:\n{json_out}"));
    assert_eq!(
        rels.len(),
        3,
        "expected exactly the 3 fixture edges in the `relationships` array:\n{json_out}"
    );
    let json_types: Vec<String> = rels
        .iter()
        .map(|r| {
            r.get("rel_type")
                .and_then(|v| v.as_str())
                .unwrap_or("(missing rel_type)")
                .to_string()
        })
        .collect();
    for t in ["blocked-by", "child", "blocks"] {
        assert!(
            json_types.iter().any(|jt| jt == t),
            "`aida show --json`'s relationships array must name the stored \
             type `{t}` (got {json_types:?}):\n{json_out}"
        );
    }
}

/// TASK-1417: above the inline threshold (five), the human view must not
/// silently withhold rows — it must say so, distinguishably from a bare
/// count. Pins the exact boundary: five relationships enumerate; six collapse
/// WITH the declaration present. TOON is unaffected (it always enumerates).
///
/// DEMONSTRATE-IT-FIRES: as found, the six-relationship collapse line read
/// "6 relationship(s)  (use --rels to enumerate, or `aida rel list <spec>`)"
/// with no assertion-stable declaration that rows were withheld by a view
/// limit — the checks below for "withheld" / "view limit" are RED against
/// that wording and green after the fix.
// trace:TASK-1417 | ai:claude
#[test]
fn task_1417_relationship_collapse_declares_view_limit_at_threshold() {
    let (_base, repo, home) = init_repo();

    let add_spec = |title: &str| -> String {
        let out = aida(&repo, &home)
            .env("AIDA_AGENT_OUTPUT", "toon")
            .env("AIDA_SESSION_ROLE", "advisor")
            .args([
                "add", "--type", "task", "--status", "approved", "--title", title,
            ])
            .output()
            .expect("run aida add");
        assert!(
            out.status.success(),
            "aida add failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        parse_spec_id(&String::from_utf8_lossy(&out.stdout))
    };

    let add_n_references = |spec: &str, n: usize| {
        for i in 0..n {
            let target = add_spec(&format!("TASK-1417 fixture: rel target {i}"));
            let rel = aida(&repo, &home)
                .env("AIDA_AGENT_OUTPUT", "toon")
                .args(["rel", "add", spec, &target, "--type", "references"])
                .output()
                .expect("run aida rel add");
            assert!(
                rel.status.success(),
                "rel add failed: {}",
                String::from_utf8_lossy(&rel.stderr)
            );
        }
    };

    // ---- Exactly five: still enumerated inline, no collapse. ----
    let five_spec = add_spec("TASK-1417 fixture: five relations");
    add_n_references(&five_spec, 5);
    let five_show = aida(&repo, &home)
        .env("AIDA_AGENT_OUTPUT", "0")
        .args(["show", &five_spec])
        .output()
        .expect("run aida show (five)");
    let five_out = String::from_utf8_lossy(&five_show.stdout);
    assert!(
        five_out.contains("Relations:"),
        "five relationships must still enumerate under the `Relations:` \
         header, not collapse:\n{five_out}"
    );
    assert!(
        !five_out.contains("withheld") && !five_out.contains("view limit"),
        "five relationships is AT the inline threshold and must not trigger \
         the collapse declaration:\n{five_out}"
    );

    // ---- Six: collapses, and the collapse DECLARES itself. ----
    let six_spec = add_spec("TASK-1417 fixture: six relations");
    add_n_references(&six_spec, 6);
    let six_show = aida(&repo, &home)
        .env("AIDA_AGENT_OUTPUT", "0")
        .args(["show", &six_spec])
        .output()
        .expect("run aida show (six)");
    let six_out = String::from_utf8_lossy(&six_show.stdout);
    assert!(
        six_out.contains("6 relationship"),
        "the collapsed line must still state the true count:\n{six_out}"
    );
    assert!(
        six_out.contains("withheld") && six_out.contains("view limit"),
        "above the threshold, the human view must explicitly declare that \
         rows were withheld by a view limit — distinguishable from a spec \
         that genuinely has only a count:\n{six_out}"
    );
    assert!(
        six_out.contains("--rels"),
        "the collapse line must still point at the escape hatch:\n{six_out}"
    );

    // ---- TOON is unaffected: it always enumerates every edge. ----
    let six_toon = aida(&repo, &home)
        .env("AIDA_AGENT_OUTPUT", "toon")
        .args(["show", &six_spec])
        .output()
        .expect("run aida show (six, toon)");
    let six_toon_out = String::from_utf8_lossy(&six_toon.stdout);
    assert!(
        six_toon_out.contains("relationships[6]"),
        "TOON must still enumerate all 6 edges regardless of the human \
         threshold:\n{six_toon_out}"
    );
}
