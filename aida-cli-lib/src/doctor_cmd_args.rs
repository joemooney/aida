//! `aida doctor_cmd` clap types, owned by `doctor_cmd.rs` and re-exported from
//! `cli.rs`. trace:STORY-1728 | ai:claude

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Maintenance + migration ops on the AIDA store.
// trace:EPIC-19 | ai:claude
#[derive(Subcommand, Debug)]
pub enum DoctorCommand {
    /// Diagnose cached compiler-wrapper failures; remove only with --fix.
    // trace:TASK-2324 | ai:codex
    RustcCache {
        /// Remove a cache that still contains a failed wrapper probe.
        #[clap(long)]
        fix: bool,
    },

    /// Find prose that may have lost backticked text to shell command
    /// substitution. Reports only; candidates require human inspection.
    // trace:TASK-190 | ai:codex
    ShellSubstitutionHoles {
        /// Omit a spec whose own documentation quotes detector examples.
        // trace:BUG-B6B1278890774EF993D2EBC327862470-015 | ai:claude
        #[clap(long, value_name = "SPEC-ID", value_parser = crate::spec_arg::spec_id)]
        exclude: Vec<String>,
    },

    /// Find bare spec ids (TYPE-100) in descriptions and comments that also
    /// name this node's local spec with the same number (TYPE-HOME-100).
    /// A bare id resolves to the agreed spec, so prose meaning the local one
    /// should use the alias form. Reports only.
    // trace:BUG-B6B1278890774EF993D2EBC327862470-036 | ai:claude
    BareIdTwins,

    /// Focused multi-agent drift diagnostic for one category.
    // trace:STORY-462 | ai:codex
    Check {
        /// Category to check, e.g. stale-leases, orphan-branches,
        /// stale-remote-branches, OBE-briefs.
        category: String,

        /// Show older completed-without-commit findings individually.
        #[clap(long)]
        all: bool,

        /// Emit machine-readable JSON.
        #[clap(long)]
        json: bool,

        /// Exit non-zero when this category has any finding, so the check can
        /// gate a scheduled job instead of only reporting. Off by default:
        /// `doctor check` stays report-only unless a caller asks to be gated.
        // trace:STORY-1422 | ai:claude
        #[clap(long)]
        fail_on_findings: bool,
    },

    /// Focused multi-agent drift heal for one category.
    // trace:STORY-462 | ai:codex
    Heal {
        /// Category to heal, e.g. stale-leases, orphan-branches,
        /// stale-remote-branches, OBE-briefs.
        category: String,

        /// Skip confirmation prompts.
        #[clap(long, short = 'y')]
        yes: bool,

        /// Permit riskier destructive fixes such as branch deletion.
        #[clap(long)]
        force: bool,

        /// Show older completed-without-commit findings individually.
        #[clap(long)]
        all: bool,

        /// Emit machine-readable JSON.
        #[clap(long)]
        json: bool,
    },

    /// Migrate this project from per-type counters (each type counts
    /// independently — every type starts back at 1) to a single global
    /// counter (one shared sequence — IDs increment across types).
    /// Allocates a new `*` block above the existing per-type blocks'
    /// max range_end, marks per-type blocks as exhausted, sets
    /// `[id_format] counter_scope = "global"`. Existing requirement
    /// spec_ids stay unchanged — only newly-added reqs use the global
    /// counter.
    // trace:EPIC-19, FR-271 | ai:claude
    // trace:TASK-487 | ai:claude
    MigrateCounterScope {
        /// Target scope. Today only `global` is supported. (Going from
        /// global back to per-type is harder — no migration path yet.)
        #[clap(long, value_parser = ["global"])]
        to: String,

        /// Print the planned changes without writing anything.
        #[clap(long)]
        dry_run: bool,

        /// Skip the y/N confirmation.
        #[clap(long, short = 'y')]
        yes: bool,

        /// Block size for the new shared block (default 1000, matching
        /// `aida init` Global default).
        #[clap(long, default_value = "1000")]
        size: u32,
    },

    /// Find and tombstone blocks whose `node_id` isn't in nodes.toml
    /// (orphaned from a clone that never registered, or from a
    /// pre-multi-node store). The block range stays reserved so other
    /// clones don't reallocate over it; `next` is bumped past `range_end`
    /// so the dispenser skips it.
    // trace:EPIC-19, EPIC-1-052 | ai:claude
    // trace:TASK-487 | ai:claude
    RepairStaleBlocks {
        /// Print what would change without writing.
        #[clap(long)]
        dry_run: bool,
        /// Skip confirmation.
        #[clap(long, short = 'y')]
        yes: bool,
    },

    /// Detect duplicate spec_ids in the orphan store — multiple YAMLs
    /// claiming the same id (legacy leftovers, imports gone wrong).
    /// Reports only; v1 doesn't auto-renumber because that would orphan
    /// trace comments and commit refs.
    // trace:EPIC-19, BUG-31 | ai:claude
    // trace:TASK-487 | ai:claude
    ScrubCollisions,

    /// Walk every requirement's `relationships` array and verify each
    /// `target_id` resolves to an existing requirement UUID. Catches
    /// dangling references from deleted reqs, bad imports, or
    /// hand-edits. With --repair, strips dangling entries.
    // trace:EPIC-19 | ai:claude
    // trace:TASK-956 | ai:claude — hidden from `doctor --help` (still runs).
    #[clap(hide = true)]
    VerifyRelationships {
        /// Strip dangling references in-place. Without this flag, the
        /// command reports only.
        #[clap(long)]
        repair: bool,
        /// Skip the y/N confirmation when --repair would write.
        #[clap(long, short = 'y')]
        yes: bool,
    },

    /// Walk source files under the project root for AIDA trace annotations
    /// and verify each spec_id resolves to an existing requirement.
    /// Catches dead trace comments left behind after a req got deleted, or
    /// simple typos. Default is read-only; pass `--strip-dangling` to
    /// remove markers pointing at unknown spec_ids (whole comment line
    /// deleted if the trace was its only content; otherwise just the
    /// trace fragment is excised).
    // trace:EPIC-19 | ai:claude
    ValidateTraceComments {
        /// Rewrite source files to remove dangling AIDA trace annotations.
        /// Lossy — comments around the marker are preserved, but the
        /// pointer itself is gone.
        // trace:EPIC-19 | ai:claude
        #[clap(long)]
        strip_dangling: bool,
        /// Print what would change without writing.
        #[clap(long)]
        dry_run: bool,
        /// Skip the y/N confirmation when --strip-dangling would write.
        #[clap(long, short = 'y')]
        yes: bool,
    },

    /// Run every diagnostic in sequence and print a unified report.
    /// Composes `aida db block verify`, repair-stale-blocks --dry-run,
    /// scrub-collisions, verify-relationships, validate-trace-comments,
    /// plus a few smaller checks. Exits non-zero if any check found a
    /// problem.
    // trace:EPIC-19 | ai:claude
    // trace:TASK-956 | ai:claude — folded into the `doctor` surface: hidden
    // from `doctor --help` (the canonical drift verb is `doctor check`), but
    // the `fsck` name still runs its own full-suite integrity report. It is
    // NOT a true alias of `doctor check` — `check` is the per-category
    // multi-agent diagnostic and requires a category arg, whereas `fsck` runs
    // the whole legacy suite with no arg, so collapsing it would drop a
    // capability. Hiding preserves the name + behavior, off `--help`.
    #[clap(hide = true)]
    Fsck,

    /// Check that STORY / BUG descriptions contain a recognized
    /// acceptance heading (`## Acceptance`, `## Verify`, `## Tests`,
    /// `## Test cases`, `## Verification` — case-insensitive). Surfaced
    /// by the review-prompt generator: missing sections produce a
    /// placeholder downstream, this lint catches them at write-time.
    /// Exits non-zero if any STORY/BUG is missing a section.
    // trace:STORY-70, STORY-67 | ai:claude
    // trace:TASK-487 | ai:claude
    ConventionCheck {
        /// Quiet mode — print only the summary line, omit per-spec rows.
        #[clap(long, short = 'q')]
        quiet: bool,
    },
}
