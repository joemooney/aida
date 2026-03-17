# Documentation Review Report

**Date**: 2026-03-17
**Scope**: All user-facing documentation (README.md, CLAUDE.md, OVERVIEW.md, docs/getting-started.md, docs/storage-modes.md, docs/multi-user-setup.md, docs/WHY-AIDA.md)

---

## Executive Summary

The documentation set is comprehensive and covers the system well, but suffers from **inconsistencies between documents** that have accumulated as features were added across sessions. The most critical issues are: contradictory backend counts, a stale README that undersells the project, stale references in admin-guide.md, GitHub integration documented before it was confirmed working, and a REQUIREMENTS.md file that exists despite CLAUDE.md explicitly forbidding it.

---

## Issues by Document

### README.md (Project Root)

**Status**: Exists but is stale and incomplete. This is the first thing any visitor sees on GitHub and it does not do the project justice.

1. **YAML listed as default storage backend** (line 58): Says `YAML -- Human-readable, git-friendly (default)`. Every other document (storage-modes.md, user-guide.md, CLAUDE.md, getting-started.md) correctly says SQLite is the default. This is a direct contradiction.

2. **Feature list is outdated and thin**. Missing major features: My Queue, My Activity, Sprint Planning, Chat, Skills Browser, Timeline view, advanced query builder, keyboard shortcuts, Dashboard view. The React Dashboard feature list (lines 67-74) covers maybe 30% of the actual capabilities.

3. **No value proposition**. The README opens with a dry one-liner. Compare this to WHY-AIDA.md which is compelling. The README needs at least a 2-3 sentence pitch.

4. **Quick Start section assumes source build only**. No Docker quickstart (the recommended path per CLAUDE.md and OVERVIEW.md). Should lead with `make docker-up` or `docker compose -f .aida/docker-compose.yml up`.

5. **Says "15 skills"** (line 83) but there are actually 21 skills in `.claude/skills/`. CLAUDE.md header section says 15, the skills list section says 16. None of these numbers match reality.

6. **No link to getting-started.md, storage-modes.md, or WHY-AIDA.md** in the documentation section. Only links to OVERVIEW.md, CLAUDE.md, user-guide.md, and plans/.

7. **Missing badges/shields** -- no CI status, no license, no crate version. Standard for a GitHub project.

8. **No mention of PostgreSQL or distributed mode** in the Quick Start.

9. **Says `docker-compose.yml`** in CLAUDE.md (line 10) but no `docker-compose.yml` exists at root. The actual file is `.aida/docker-compose.yml`. OVERVIEW.md correctly references the `.aida/` path.

---

### CLAUDE.md

1. **Backend count inconsistency**: Line 58 says "four storage backends" and lists YAML, SQLite, PostgreSQL, Git. The system prompt version (injected into this conversation) says "three storage backends" and does not list Git. The actual current state is 5 modes (YAML, SQLite, PostgreSQL, Git Worktree, Git Sibling) per storage-modes.md.

2. **Skill count inconsistency**: Line 109 says "15 workflow skills", line 216 says "16 skills". The actual count of `aida-*` files in `.claude/skills/` is 21. The skill list on lines 219-242 enumerates exactly 16 skills but is missing: `/aida-architecture`, `/aida-decompose`, `/aida-glossary`, `/aida-grill`, `/aida-triage`.

3. **Docker quickstart references `docker-compose.yml`** (line 10) without path qualification. The file is actually `.aida/docker-compose.yml`. There is no root-level `docker-compose.yml`.

4. **Project name inconsistency**: Line 7 says "AI Design Assistant". Other documents call it "AIDA -- AI Design Assistant". The name should be consistent everywhere.

5. **Distributed Mode section** (lines 77-94): Documents `aida db merge-gate`, `aida db sync --pull --push`, `aida db status` commands. These should be verified as actually implemented. This section is well-written but only present in CLAUDE.md and storage-modes.md -- the README doesn't mention distributed mode at all.

6. **REQUIREMENTS.md contradiction**: Line 50 says "Do NOT maintain a separate REQUIREMENTS.md file." but `/home/joe/ai/aida/REQUIREMENTS.md` (237 lines) exists in the repo. Either delete the file or remove the instruction.

---

### OVERVIEW.md

1. **Docker quickstart path**: Line 349 says `make docker-up  # or: docker compose -f .aida/docker-compose.yml up`. This is correct. However, line 349 also presupposes cloning from `github.com/yourusername/aida.git` -- the placeholder URL should be updated to the real repo URL (`github.com/joemooney/aida.git`).

2. **Extremely long document** (726 lines). Much of it duplicates information better placed in other docs. The "Use Cases & Tutorials" section (lines 412-715) is 300 lines that could live in a separate `docs/tutorials.md` or in the user guide. OVERVIEW.md should be a concise orientation document, not the everything file.

3. **GitLab integration described in detail** (lines 208-250) but GitHub integration is mentioned only in passing. The getting-started.md documents `aida github` commands, suggesting GitHub integration exists. If so, OVERVIEW.md should document it. If not, getting-started.md should remove it.

4. **"Three Interfaces" section** lists CLI, Web Dashboard, Desktop App. The desktop app section is detailed but the WASM browser client section (lines 162-178) describes a fourth interface. The naming is confusing.

5. **gRPC server port**: Line 397 says `--port 50051` for the gRPC server. This is separate from the REST port 8080. The relationship between these ports is unclear. Some documents imply both are always running; others imply you choose one.

6. **Dead/incorrect references**: Line 722 says `docs/user-guide.md: Comprehensive user documentation`. This is accurate. Line 723 says `docs/admin-guide.md: Storage administration...` -- this file exists but says the system supports only "two storage backends" (YAML and SQLite), which is stale now that PostgreSQL, Git Worktree, and Git Sibling exist.

---

### docs/getting-started.md

1. **GitHub integration documented as if implemented** (lines 193-209). Commands like `aida github config`, `aida github push`, `aida github pull` are documented. The code does have a `handle_github_command` in main.rs, so this appears to be implemented, but the feature is not mentioned in OVERVIEW.md or the README. Either it should be added there too, or if it's experimental/incomplete, it should be marked as such here.

2. **Pre-built binary section** (lines 29-38) says "Pre-built binaries will be available once GitHub releases are set up." This note is honest, but should be either removed when releases are set up or made more prominent (currently it reads like the binaries exist).

3. **Docker section** (lines 43-53) says "Docker image will be available once CI publishes it." Same issue as above. This is aspirational text that could confuse someone trying to follow the instructions literally.

4. **YAML backend note** (line 27): `aida init` section says it creates `requirements.db` (SQLite). But the YAML section under storage-modes.md says `aida init` creates `requirements.yaml` "if no requirements.db exists." This conditional behavior is unclear in getting-started.md which states it creates SQLite unconditionally.

5. **Quick Reference table** includes distributed commands (`aida db status`, `aida db merge-gate`, `aida db sync`) which is good, but these are mixed in with basic commands, making the table feel overwhelming for a getting-started guide.

6. **Uninstall section** mentions `.aida/` directory (line 241) but earlier in the doc it only mentions `requirements.db`, `.mcp.json`, `.claude/skills/`, and `docs/plans/` as created by `aida init`. The `.aida/` directory is a distributed-mode artifact. The discrepancy is confusing.

7. **Missing**: No mention of `make docker-up` as a quickstart option in the "What's Next?" section, despite it being the recommended path in CLAUDE.md and OVERVIEW.md.

---

### docs/storage-modes.md

**This is the best-written document in the set.** Clear, well-structured, with good decision flowchart and comparison matrix.

1. **Minor issue -- YAML init behavior** (line 27): Says `aida init` creates `requirements.yaml` "if no requirements.db exists." This conditional logic is only documented here and may surprise users. Other docs just say `aida init` creates SQLite.

2. **Git Sibling mode** (line 197): Documents `aida init --distributed --sibling [--registry-remote ...]`. This flag should be verified as implemented. It's documented here and nowhere else.

3. **Combining Modes section** (lines 302-330): The "Git store + PostgreSQL cache" example (line 308) shows `aida-server --database .aida-store --rest-port 8080`. This implies aida-server can read directly from a git store path, which may or may not be implemented. If aspirational, should be marked as such.

4. **GitHub integration** in "Combining Modes" section (lines 323-330) uses `aida github config` and `aida github push` -- consistent with getting-started.md but absent from OVERVIEW.md and README.

5. **Minor**: The "Last updated: 2026-03-16" date should be kept accurate on edits.

---

### docs/multi-user-setup.md

1. **Build features flag** (line 13): Says `cargo build --features postgres,gitlab,github`. If GitHub integration is behind a feature flag, this is important information that should also appear in getting-started.md and README.md.

2. **Hardcoded IP addresses** (lines 73, 86-89, 98, etc.): Uses `192.168.179.180` throughout. This is clearly a specific dev machine. Should use a placeholder like `<server-ip>` or `192.168.x.x` with a note to substitute your own.

3. **Requirement count in architecture diagram** (line 294): Says "354+ requirements". This is a snapshot that will become outdated immediately. Should say "N requirements" or omit the number.

4. **Production deployment section** (lines 167-208): References `aida-api.joemooney.com` and `aida.joemooney.com` -- these are specific to the developer's personal setup. For a user-facing doc, this should be genericized (e.g., `aida-api.example.com`).

5. **Authentication section** (lines 198-208): Documents `--auth-mode pin|apikey|oidc`. Good to document, but should note whether these are implemented or planned. OIDC in particular is a significant feature.

6. **Switching Between Modes section** (lines 213-235): The "Distributed to Centralized" migration (line 230) suggests `aida db migrate --from yaml --to postgres`. But the distributed store is not plain YAML -- it's sharded YAML files in a git repo. Is this command aware of that? If it requires an intermediate export step, document it.

---

### docs/WHY-AIDA.md

**This is excellent.** Well-written, honest, and compelling. Minor issues:

1. **Says "15 skills"** (lines 85, 225). Should be updated to match the actual count (21).

2. **Says "Three storage backends"** (line 102). Should be updated to reflect all 5 modes, or say "multiple storage backends" as storage-modes.md does.

3. **Section 6 of "Where AIDA Needs to Grow"** (line 251): Says "There's no landing page, no 'getting started in 5 minutes' tutorial." But getting-started.md now exists and its tagline is literally "in under 5 minutes." This section should be updated to reflect that this gap has been partially addressed.

4. **GitLab mentioned, GitHub absent** (line 103, 148): "GitLab integration with bidirectional sync" is listed as a feature. The competitive analysis mentions GitHub integration as something that "would be natural." But the codebase and getting-started.md suggest GitHub integration already exists. If so, WHY-AIDA.md should mention it alongside GitLab.

---

## Cross-Document Inconsistencies

### 1. Storage Backend Count

| Document | Count | Backends Listed |
|----------|-------|-----------------|
| README.md | 3 (implied) | YAML (default!), SQLite, PostgreSQL |
| CLAUDE.md (project) | 4 | YAML, SQLite, PostgreSQL, Git |
| CLAUDE.md (system prompt) | 3 | YAML, SQLite, PostgreSQL |
| storage-modes.md | 5 | YAML, SQLite, PostgreSQL, Git Worktree, Git Sibling |
| user-guide.md | 3 | YAML, SQLite, PostgreSQL |
| admin-guide.md | 2 | YAML, SQLite |
| WHY-AIDA.md | 3 | YAML, SQLite, PostgreSQL |
| OVERVIEW.md | 3+ | Lists YAML, SQLite, PostgreSQL; mentions Git in "Distributed Mode" separately |

**Recommendation**: Standardize on "5 storage modes" (matching storage-modes.md) or "3 core backends + 2 distributed modes" in all documents. At minimum, every document that lists backends should include PostgreSQL and mention that distributed git-based modes exist.

### 2. Skill Count

| Document | Count |
|----------|-------|
| README.md | 15 |
| CLAUDE.md (line 109) | 15 |
| CLAUDE.md (line 216) | 16 |
| WHY-AIDA.md | 15 |
| Actual files in .claude/skills/ | 21 |

**Recommendation**: Update all to 21 or use "20+" to future-proof.

### 3. Default Storage Backend

| Document | Says Default Is |
|----------|-----------------|
| README.md | YAML |
| CLAUDE.md | SQLite (implied by `requirements.db`) |
| getting-started.md | SQLite |
| storage-modes.md | SQLite |
| user-guide.md | SQLite |
| admin-guide.md | YAML |

**Recommendation**: README.md and admin-guide.md are wrong. SQLite is the default. Fix these.

### 4. Docker Compose File Location

| Document | Path Referenced |
|----------|----------------|
| CLAUDE.md | `docker-compose.yml` (no path, implies root) |
| OVERVIEW.md | `.aida/docker-compose.yml` (correct) |
| Makefile | `.aida/docker-compose.yml` (correct) |

**Recommendation**: CLAUDE.md should say `.aida/docker-compose.yml` or `make docker-up`.

### 5. GitHub Integration Status

Documented in getting-started.md and storage-modes.md but not mentioned in README.md, OVERVIEW.md features list, or WHY-AIDA.md. Code exists in `aida-cli/src/main.rs`. Either add it everywhere or mark it as experimental where it appears.

---

## Missing Content Gaps

1. **No CHANGELOG.md** -- users have no way to know what's new between versions.

2. **No architecture diagram for newcomers** -- OVERVIEW.md has a project structure tree but no high-level architecture diagram showing how CLI/server/web/desktop/MCP relate to each other and to storage backends.

3. **No "How AIDA compares" quick table** in README.md. WHY-AIDA.md has detailed competitive analysis but the README should have a 3-row summary.

4. **No screenshot or demo GIF** in README.md. For a tool with a web dashboard, kanban board, and sprint planning, visuals would dramatically improve first impressions.

5. **No "Contributing" section** in README.md.

6. **No link from README.md to WHY-AIDA.md** -- the most compelling document in the set is undiscoverable from the main entry point.

7. **admin-guide.md is significantly stale** -- only covers YAML and SQLite, missing PostgreSQL, Git, and distributed modes entirely. Either update it or redirect to storage-modes.md and multi-user-setup.md.

---

## Recommended Restructuring

### Priority 1 (Critical)
- [ ] **Fix README.md**: Rewrite to be the compelling entry point. Add Docker quickstart, correct the default backend, link to WHY-AIDA.md, add a screenshot, update skill count, mention all 5 storage modes.
- [ ] **Fix backend count everywhere**: Standardize on "5 storage modes" or "3 backends + 2 distributed modes".
- [ ] **Fix skill count everywhere**: Update to 21 (or count dynamically).
- [ ] **Fix YAML-as-default in README.md and admin-guide.md**: SQLite is the default.
- [ ] **Resolve REQUIREMENTS.md contradiction**: Delete the file or remove the CLAUDE.md instruction.

### Priority 2 (Important)
- [ ] **Update admin-guide.md**: Add PostgreSQL and distributed modes, or deprecate in favor of storage-modes.md + multi-user-setup.md.
- [ ] **Genericize multi-user-setup.md**: Replace hardcoded IP `192.168.179.180` with `<server-ip>`, remove specific requirement count from architecture diagram, genericize domain names.
- [ ] **Add GitHub integration to OVERVIEW.md and README.md** (if it's stable).
- [ ] **Fix docker-compose.yml reference in CLAUDE.md**: Qualify with `.aida/` path.

### Priority 3 (Nice to Have)
- [ ] **Extract tutorials from OVERVIEW.md**: Move Use Cases 1-6 to a dedicated `docs/tutorials.md`. OVERVIEW.md should be under 300 lines.
- [ ] **Add architecture diagram**: A simple box-and-arrow diagram showing CLI/server/web/desktop/MCP and their relationships to storage backends.
- [ ] **Add screenshot(s) to README.md**: Kanban board, dashboard, or sprint planning view.
- [ ] **Update WHY-AIDA.md section 6**: Acknowledge that getting-started.md now exists.
- [ ] **Mark aspirational features**: Pre-built binaries, Docker Hub image, OIDC auth -- mark these as "planned" or "coming soon" where documented.

---

## Related Requirements

- Ensure AIDA requirements are created for the Priority 1 and Priority 2 items above.

## Status

In Progress
