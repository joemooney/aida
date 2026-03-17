# Prior Art: Storing Issue/Bug Tracking Data Inside Git

**Date**: 2026-03-16
**Type**: Research / Spike
**Status**: Completed

## Summary

Research into existing tools and approaches for storing structured metadata (issues, bugs, reviews, wiki) inside git repositories. This survey covers six dedicated tools, git's built-in notes mechanism, Fossil's alternative approach, and the orphan branch + worktree pattern. The goal is to understand what has been tried, what worked, what failed, and what trade-offs exist for AIDA's own git-native storage strategy.

---

## 1. git-appraise (Google)

**Repository**: [google/git-appraise](https://github.com/google/git-appraise)
**Language**: Go
**Status**: Low activity; last release v0.7 (April 2021), 306 commits total. Effectively dormant.

### Storage Mechanism: Git Notes

git-appraise stores all code review data in **git notes** under dedicated refs:

| Ref | Purpose |
|-----|---------|
| `refs/notes/devtools/reviews` | Code review requests (annotate initial commits) |
| `refs/notes/devtools/discuss` | Human comments on reviews |
| `refs/notes/devtools/analyses` | Automated static analysis results |
| `refs/notes/devtools/ci` | Build/test status results |

Each data item is written as a **single JSON line** per note. This design enables automatic merging via the `cat_sort_uniq` merge strategy -- when two people add different review comments, the notes can be concatenated, sorted, and deduplicated without conflicts.

### Multi-User / Distributed Edits

- Fully decentralized: every developer has their own copy of review history
- Sync via `git appraise push` / `git appraise pull`
- Automatic merging using `cat_sort_uniq` -- no manual conflict resolution needed for append-only data
- Works with any git hosting provider; no server-side components

### What Worked

- **Simplicity**: Notes are lightweight and don't pollute the working tree
- **Automatic merging**: The one-JSON-line-per-entry format with `cat_sort_uniq` is elegant
- **Zero infrastructure**: Works with any git remote

### What Failed / Limitations

- **Limited adoption**: Never gained traction outside Google
- **Poor discoverability**: `git notes` are not visible in normal `git log` output by default
- **Not pushed by default**: `git push` does not include notes refs unless explicitly configured
- **Limited query capability**: No indexing; searching means scanning all notes
- **Scope too narrow**: Only code reviews, not general issue tracking

### Key Insight

The one-line-JSON + `cat_sort_uniq` pattern is the best practical approach for conflict-free appending in git notes. It works well for append-only data (comments, reviews) but poorly for mutable state (status changes, priority updates).

---

## 2. git-bug

**Repository**: [git-bug/git-bug](https://github.com/git-bug/git-bug)
**Language**: Go
**Status**: **Actively maintained**. 2,429 commits, v0.10.1 released May 2025. The most mature tool in this category.

### Storage Mechanism: Custom Git Objects Under Special Refs

git-bug stores data as **git objects (blobs, trees, commits) under custom refs** -- specifically `refs/bugs/<id>`. This is neither git notes nor orphan branches; it creates a parallel commit DAG that does not appear in the working tree or normal branch history.

The storage hierarchy:

```
refs/bugs/<entity-id>
  └── Commit chain
       └── Tree
            ├── /ops      (blob: JSON array of operations)
            └── /media     (optional: attached media blobs)
```

Each entity (bug) is modeled as a **chain of commits**, where each commit contains an `OperationPack` -- a JSON array of edit operations. The commit chain grows as operations are appended.

### Data Model: Operation-Based CRDTs

This is the most sophisticated approach in the survey. git-bug uses an **operation-based CRDT** (Conflict-free Replicated Data Type):

- Entities are not stored as final state but as **a series of edit operations**
- Operations include: CreateOp, SetTitleOp, SetStatusOp, AddCommentOp, etc.
- Each operation carries a **Lamport logical clock** value for causal ordering
- Final state is computed by replaying operations in deterministic order

**Conflict resolution**:
1. Load all commits and OperationPacks from the DAG
2. Validate Lamport clock ordering (parents < children)
3. For sequential operations: order by Lamport clock
4. For concurrent operations (same Lamport value): order by lexicographic OperationPack identifier
5. Result: **deterministic total ordering** across all distributed replicas

### Multi-User / Distributed Edits

- Sync via standard `git push` / `git pull` on the `refs/bugs/*` refs
- Bridges for syncing with GitHub Issues and GitLab Issues
- Concurrent edits produce a DAG that is automatically linearized via Lamport clocks
- No manual conflict resolution ever needed

### What Worked

- **CRDT model is the gold standard**: Truly conflict-free distributed editing
- **No working tree pollution**: Data lives in refs, invisible to normal git operations
- **Rich functionality**: Labels, milestones, comments, status -- full issue tracker
- **Web UI and TUI**: Terminal and browser interfaces included
- **Bridge system**: Bidirectional sync with GitHub/GitLab
- **Fast**: Millisecond-level search and queries

### What Failed / Limitations

- **Complexity**: The CRDT + Lamport clock system is hard to understand and debug
- **Custom tooling required**: Cannot inspect data with standard git commands
- **Ref proliferation**: Each bug creates its own ref; large projects accumulate thousands
- **No worktree usage**: Does not use git worktrees
- **Push configuration**: Like notes, custom refs require explicit push/fetch configuration

### Key Insight

The operation-based CRDT approach solves the fundamental problem of distributed mutable state in git. Rather than trying to merge conflicting final states, you merge append-only operation logs and deterministically replay them. This is the most robust approach discovered in this research.

---

## 3. Fossil

**Repository**: [fossil-scm.org](https://fossil-scm.org)
**Language**: C
**Status**: **Actively maintained**. Created by D. Richard Hipp (creator of SQLite).

### Storage Mechanism: SQLite Database with Artifact Model

Fossil is not git-based -- it is a completely separate VCS that includes bug tracking, wiki, forum, and chat as built-in features. However, it provides the strongest contrast for understanding design trade-offs.

**Architecture**:
- All data (source, tickets, wiki, forum, chat) stored in a **single SQLite database**
- Data is modeled as an **unordered set of artifacts** (content-addressed by SHA1/SHA3-256)
- Two artifact types: **content artifacts** (files) and **structural artifacts** (manifests, ticket changes, wiki edits, etc.)
- Structural artifacts use a rigid **card-based text format** (single-character type codes in lexicographic order)
- SQLite tables serve as **performance indexes** -- they are fully rebuildable from artifacts via `fossil rebuild`

**Synchronization**:
- Repositories sync by computing the **union of their artifact sets**
- "Autosync" mode reduces unnecessary forking
- Ticket changes are artifacts too -- they sync the same way as code changes

### What Worked

- **Unified model**: One tool, one database, everything integrated
- **Artifact immutability**: Append-only design prevents data loss
- **Rebuild capability**: All relational indexes are derived, not authoritative
- **Extreme efficiency**: Runs on a Raspberry Pi or $5/month VPS
- **Longevity-focused**: Artifact format designed to be readable "by people not yet born"

### What Failed / Limitations

- **Not git**: Cannot be used with existing git workflows, GitHub, etc.
- **Centralized tendency**: While technically distributed, the autosync model encourages hub-and-spoke
- **Limited ecosystem**: Small community compared to git
- **Monolithic**: Cannot pick and choose features

### Key Insight

Fossil proves that integrating project metadata with version control is viable and even desirable -- the question is whether you can achieve similar integration within git's architecture rather than replacing git entirely. Fossil's artifact-as-append-only-event model is philosophically identical to git-bug's operation-based CRDT.

---

## 4. SIT (Serverless Information Tracker)

**Repository**: [sit-fyi/sit](https://github.com/sit-fyi/sit)
**Language**: Rust
**Status**: Early adopter stage as of last update. 1,138 commits. Last releases around October 2018. Appears dormant.

### Storage Mechanism: Plain Files in `.sit/` Directory

SIT takes the most straightforward approach: **plain files in a `.sit/` directory** within the repository. No special git features used -- just regular tracked files.

- Each "item" (issue) gets a directory under `.sit/`
- Each "record" (event/change) is a subdirectory within the item, named by hash
- Records contain files representing individual fields/properties
- State is computed by replaying records in order (similar to event sourcing)

### Multi-User / Distributed Edits

- Merge-friendly by design: records are append-only files in unique directories
- Adding a record = adding a new subdirectory, which never conflicts with git merge
- Sync via any file transfer mechanism: git, Dropbox, USB drives, etc.
- No custom git plumbing needed

### What Worked

- **Extreme simplicity**: Just files in directories
- **Transport agnostic**: Works with any file sync mechanism, not just git
- **Inspectable**: Can browse data with `ls` and `cat`
- **Merge-friendly**: Directory-per-record means no merge conflicts

### What Failed / Limitations

- **Working tree pollution**: `.sit/` directory lives alongside source code
- **Performance**: File-per-field creates many small files; poor for large projects
- **Git bloat**: Every record is a tracked file, inflating repository size over time
- **No indexing**: Query performance depends on filesystem traversal
- **Abandoned**: Project appears dormant since 2018

### Key Insight

The "just use files" approach is the simplest but scales poorly. The directory-per-record pattern does solve merge conflicts elegantly, but at the cost of repository bloat and working tree pollution.

---

## 5. git-dit (Distributed Issue Tracker)

**Repository**: [neithernut/git-dit](https://github.com/neithernut/git-dit)
**Language**: Rust
**Status**: Pre-1.0. Last release v0.4.0 (September 2017). 924 commits. Appears dormant.

### Storage Mechanism: Free-Standing Commits Under Special Refs

git-dit uses a unique approach: **issues and comments are stored as git commits that are not part of any branch**. These commits have no tree (or an empty tree) and carry their content in the commit message.

**Reference structure**:
- `refs/dit/<issue-hash>/head` -- points to the current state of each issue
- `refs/dit/<issue-hash>/leaves/` -- tracks leaf commits to prevent garbage collection

**Message tree architecture**:
- The initial commit of an issue is a free-standing commit (no parent)
- Comments/replies are commits whose first parent is the commit they reply to
- Metadata (status, type) uses git trailer conventions (`Dit-status: open`, `Dit-type: bug`)
- Maintainers can update the `head` ref to establish accepted discussion state

### Multi-User / Distributed Edits

- Sync via `git dit push` / `git dit pull`
- No server-side software needed
- Concurrent edits create parallel commit chains that can be independently traversed
- No automatic conflict resolution for status changes (last-writer-wins via head ref)

### What Worked

- **Commit-as-message is natural**: Leverages git's existing commit format
- **No working tree pollution**: Data lives entirely in refs
- **Lightweight**: No blobs or trees needed (commit message carries all data)
- **Discussion threading**: Parent-child commit relationships naturally model threaded discussion

### What Failed / Limitations

- **Unconventional**: Using commits for data (not code) feels like an abuse of git
- **Limited metadata**: Trailer-based metadata is unstructured and hard to query
- **No rich data**: Only plain text in commit messages
- **Ref proliferation**: Two refs per issue minimum
- **Abandoned**: No activity since 2017

### Key Insight

Using git commits as messages is elegant for discussion-oriented data but poor for structured metadata. The trailer-based approach (`Dit-status: open`) is too limited for complex issue tracking.

---

## 6. Bugs Everywhere

**Repository**: [bugseverywhere/bugs-everywhere](https://github.com/bugseverywhere/bugs-everywhere)
**Language**: Python
**Status**: **Dormant**. Last release v1.1.1 (June 2013). Python 2.7 only.

### Storage Mechanism: Tracked Files in Repository

Bugs Everywhere stores bug data as **regular tracked files** within the repository, alongside source code. Bugs get globally unique identifiers (UUIDs) instead of sequential numbers.

**Supported VCS**: Git, Mercurial, Bazaar, Darcs, Arch, Monotone -- and even standalone (no VCS).

### Multi-User / Distributed Edits

- Bug status is branch-specific: a bug can be "fixed" in one branch but "open" in another
- Merge conflicts handled by the underlying VCS's merge mechanism
- No special conflict resolution for bug metadata

### What Worked

- **VCS-agnostic**: Works with many version control systems
- **Branch-relative state**: Bug status that varies by branch is conceptually correct
- **Simple model**: Just files tracked by your VCS

### What Failed / Limitations

- **Working tree pollution**: Bug files live alongside code
- **Merge conflicts**: VCS merges of bug files can produce conflicts
- **No active maintenance**: Abandoned since 2013
- **Python 2 only**: Cannot run on modern Python
- **Poor scale**: File-per-bug with VCS tracking becomes unwieldy

### Key Insight

The branch-relative bug state concept (a bug is fixed in `release` but open in `main`) is interesting and unique. No other tool in this survey preserves this property. However, it also means bug state fragments across branches, making it hard to get a unified view.

---

## 7. Git Notes (`refs/notes/`)

Git's built-in mechanism for attaching metadata to objects.

### Storage Architecture

- Notes are stored as **regular git blobs** organized in a tree under a notes ref
- Default ref: `refs/notes/commits`; custom refs supported (e.g., `refs/notes/reviews`)
- Each note is keyed by the object ID it annotates
- The notes tree uses a **fan-out directory structure** for performance: `bf/fe/30/.../680d5a`
- Each change to a note creates a new commit at the notes ref, providing full history

### Merge Strategies

| Strategy | Behavior | Best For |
|----------|----------|----------|
| `manual` (default) | Conflict worktree for resolution | Complex metadata |
| `ours` | Keep local version | Authoritative source |
| `theirs` | Keep remote version | Subordinate replica |
| `union` | Concatenate both versions | Append-only logs |
| `cat_sort_uniq` | Concatenate, sort, deduplicate | Line-based structured data |

### Limitations

- **Not pushed by default**: Requires explicit refspec configuration
- **One note per object**: Cannot attach multiple independent notes to the same object under the same ref (must use separate refs)
- **Object-anchored**: Notes annotate existing git objects; cannot store free-standing data
- **Poor discoverability**: Not shown by default in `git log`
- **Rewrite fragility**: Notes can be lost during rebase/amend unless `notes.rewrite.*` is configured
- **No query mechanism**: Finding notes requires scanning the entire notes tree

### Best Practices

- Use separate refs for different note types (`refs/notes/reviews`, `refs/notes/ci`)
- Use line-based formats with `cat_sort_uniq` for conflict-free appending
- Configure `notes.displayRef` for visibility in `git log`
- Enable `notes.rewrite.rebase` and `notes.rewrite.amend` to preserve notes across history rewrites

### Key Insight

Git notes are designed for **annotating existing objects**, not for storing free-standing structured data. They work well for metadata that naturally associates with commits (reviews, CI results, sign-offs) but poorly for independent entities like issues or requirements.

---

## 8. The Orphan Branch + Worktree Pattern

### How It Works

An **orphan branch** is a branch with no shared history with other branches in the repository. Combined with **git worktrees**, it enables maintaining completely separate content alongside the main codebase:

```bash
# Create an orphan branch for metadata
git worktree add --orphan .aida-data

# This creates:
# - A new worktree at .aida-data/
# - An unborn branch with empty index
# - No shared history with main
```

### Known Users of This Pattern

- **GitHub Pages (`gh-pages`)**: The original and most widespread use of orphan branches. Build output is pushed to `gh-pages`, which has no shared history with the source branch. External CI tools commit compiled site files to this branch.
- **Coverage reports**: Some CI systems store coverage data on orphan branches
- **Release artifacts**: Binary release artifacts sometimes stored on orphan branches

No issue tracking tool in this survey uses the orphan branch + worktree pattern.

### Advantages Over Git Notes

| Aspect | Orphan Branch | Git Notes |
|--------|--------------|-----------|
| **Data structure** | Arbitrary files/directories | Single blob per annotated object |
| **Query capability** | Can use filesystem tools, databases | Must scan notes tree |
| **Independence** | Free-standing data | Must annotate existing objects |
| **Working tree** | Full directory tree via worktree | No working tree presence |
| **Push/fetch** | Standard branch push/fetch | Requires explicit refspec |
| **Merge** | Standard git merge | Limited strategies |
| **History** | Full commit history, diffs | Commit history on notes ref |
| **Tooling** | Standard git commands work | Requires `git notes` commands |

### Advantages Over Custom Refs (git-bug approach)

| Aspect | Orphan Branch | Custom Refs (`refs/bugs/*`) |
|--------|--------------|---------------------------|
| **Complexity** | Single branch, standard git | One ref per entity |
| **Push/fetch** | Standard branch operations | Requires refspec config |
| **Inspection** | `git log`, `git diff` work | Custom tooling required |
| **Indexing** | Can include SQLite/index files | Must build external index |
| **Scale** | Single ref, unlimited content | Thousands of refs |
| **Atomicity** | Single commit = atomic update | Per-entity commits |

### Advantages Over Tracked Files (SIT/BE approach)

| Aspect | Orphan Branch | Tracked Files |
|--------|--------------|---------------|
| **Working tree** | Separate worktree (or hidden) | Pollutes main working tree |
| **Git history** | Independent history | Interleaved with code history |
| **Branch diffs** | Clean code-only diffs | Metadata noise in diffs |
| **`.gitignore`** | Not needed | Must manage exclusions |
| **Checkout speed** | Doesn't affect main checkout | Increases checkout size |

### Disadvantages of Orphan Branch

- **Not a standard pattern** for metadata: Teams may find it confusing
- **Merge complexity**: Branch merges require understanding the separate history
- **Worktree management**: Additional worktree must be created/managed
- **CI/CD complexity**: Must handle the extra branch in automation
- **No CRDT semantics**: Still need a strategy for concurrent edits (unlike git-bug's approach)

---

## 9. Comparative Analysis

### Storage Approaches Ranked by Maturity

| Tool | Mechanism | Conflict Strategy | Status | Working? |
|------|-----------|-------------------|--------|----------|
| **git-bug** | Custom refs + CRDT ops | Lamport clock ordering | Active | Yes |
| **Fossil** | SQLite + artifacts | Set union | Active | Yes (not git) |
| **git-appraise** | Git notes | `cat_sort_uniq` | Dormant | Partial |
| **git-dit** | Free commits + refs | Last-writer-wins | Dormant | No |
| **SIT** | Tracked files | Directory-per-record | Dormant | No |
| **Bugs Everywhere** | Tracked files | VCS merge | Dead | No |

### Why Most Failed

1. **Insufficient conflict resolution**: Tools that relied on simple merge or last-writer-wins could not handle real distributed workflows
2. **Working tree pollution**: Tools that stored data as tracked files created friction with developers
3. **Push/fetch friction**: Tools using git notes or custom refs required manual configuration to sync
4. **Limited querying**: Without indexing, search was too slow for real projects
5. **Narrow scope**: Some tools (git-appraise) only solved one use case
6. **Ecosystem competition**: GitHub Issues, Jira, etc. are "good enough" and require zero setup

### What Survived and Why

Only **git-bug** remains actively maintained, and its success factors are:
- Operation-based CRDT eliminates conflict resolution entirely
- Rich feature set (labels, milestones, web UI, bridges)
- GitHub/GitLab bridges provide migration path and hybrid workflow
- No working tree pollution
- Millisecond-level performance

---

## 10. Recommendations for AIDA

Based on this research, key design principles for storing AIDA requirements data in git:

### Storage Approach: Orphan Branch with SQLite

Use an **orphan branch** (e.g., `aida-data`) to store the requirements database and metadata, accessed via a **git worktree**:

```
main branch:          source code, CLAUDE.md, etc.
aida-data branch:     requirements.db, history, attachments
```

**Why orphan branch over alternatives**:
- Unlike git notes: supports arbitrary structured data, not object-anchored
- Unlike custom refs (git-bug): single branch instead of thousands of refs, standard push/fetch
- Unlike tracked files (SIT/BE): no working tree pollution, clean code diffs
- Unlike Fossil: stays within git ecosystem

### Conflict Resolution: Operation Log (Inspired by git-bug)

For multi-user scenarios, consider an **operation log** approach:
- Store each edit as an append-only operation record
- Use Lamport-style logical clocks for causal ordering
- Deterministic replay produces consistent state across replicas
- SQLite remains the runtime query engine; operation log is the sync format

### Practical Considerations

1. **Push/fetch**: Orphan branch pushes/fetches with standard `git push origin aida-data`
2. **Worktree**: Use `git worktree add .aida aida-data` for local access (add `.aida` to `.gitignore`)
3. **Atomicity**: Each sync = one commit on the orphan branch
4. **Inspection**: Data is viewable with standard git commands (`git log aida-data`, `git diff`)
5. **CI-friendly**: Standard branch; no special refspec configuration
6. **Fallback**: Can always export to YAML for human-readable diffing

### What to Avoid

- **Do not use git notes**: Too limited, too hidden, poor push/fetch defaults
- **Do not use one-ref-per-entity**: Ref proliferation makes repo management painful
- **Do not store in tracked files on main**: Working tree pollution, noisy diffs, interleaved history
- **Do not ignore conflict resolution**: Any multi-user scenario needs a real strategy

---

## Related Requirements

- SPIKE-0006: Git scaling investigation
- Architecture decisions for distributed AIDA

## References

- [google/git-appraise](https://github.com/google/git-appraise) -- Code review in git notes
- [git-bug/git-bug](https://github.com/git-bug/git-bug) -- Distributed bug tracker, CRDT model
- [fossil-scm.org](https://fossil-scm.org) -- Integrated VCS + project management
- [sit-fyi/sit](https://github.com/sit-fyi/sit) -- File-based serverless tracker
- [neithernut/git-dit](https://github.com/neithernut/git-dit) -- Issues as git commits
- [git-scm.com/docs/git-notes](https://git-scm.com/docs/git-notes) -- Git notes reference
- [git-scm.com/docs/git-worktree](https://git-scm.com/docs/git-worktree) -- Git worktree reference
