# Plan: Rename `aida-gui` to `aida-desktop`

## Related Requirements
- General codebase hygiene / naming consistency

## Status
Completed

## Context

The web dashboard (`aida-web-react`) is now the primary UI, making `aida-gui` ambiguous. Renaming to `aida-desktop` aligns with how docs already describe it ("Desktop App") and parallels the naming: `aida` (CLI), `aida-server`, `aida-desktop`, `aida-web-react`.

---

## Phase 1: Directory + Cargo Rename (build-critical)

### 1a. Rename the directory
```
mv aida-gui/ aida-desktop/
```

### 1b. `aida-desktop/Cargo.toml` (was `aida-gui/Cargo.toml`)
- `name = "aida-gui"` → `name = "aida-desktop"` (package name)
- `name = "aida-gui"` → `name = "aida-desktop"` (binary name in `[[bin]]`)

### 1c. Root `Cargo.toml`
- Workspace member `"aida-gui"` → `"aida-desktop"`

### 1d. `aida-web/Cargo.toml`
- Dependency `aida-gui = { path = "../aida-gui" ... }` → `aida-desktop = { path = "../aida-desktop" ... }`

### 1e. `pnpm-workspace.yaml`
- `- 'aida-gui'` → `- 'aida-desktop'`

---

## Phase 2: Rust source `use aida_gui::` → `use aida_desktop::`

### 2a. `aida-desktop/src/main.rs`
- `use aida_gui::RequirementsApp` → `use aida_desktop::RequirementsApp`
- Help text strings: `"aida-gui"` → `"aida-desktop"` (lines 40, 60, 73-76)

### 2b. `aida-web/src/lib.rs`
- `pub use aida_gui::storage::proto` → `pub use aida_desktop::storage::proto`
- Comment updates

### 2c. `aida-web/src/client.rs`
- `pub use aida_gui::storage::{GrpcStorageClient, proto as shared_proto}` → `aida_desktop::`
- Comment updates

### 2d. `aida-web/src/app.rs`
- All `aida_gui::storage::proto::*` → `aida_desktop::storage::proto::*`
- All `aida_gui::ui::*` → `aida_desktop::ui::*`
- (Bulk replace `aida_gui::` → `aida_desktop::`)

### 2e. Internal `aida-desktop/` source files (comments and strings only)
- `src/lib.rs` line 39: comment mentioning `aida-gui`
- `src/app.rs`: `cargo build -p aida-gui` → `cargo build -p aida-desktop` (line ~4442)
- `src/app.rs`: `"aida-gui"` fallback strings → `"aida-desktop"` (lines ~6476, 6532, 6586)
- `src/app.rs`: `"aida-gui --file {}"` → `"aida-desktop --file {}"` (lines ~6768, 9228)
- `src/storage/embedded.rs`: error message referencing `aida-gui`
- `src/ui/mod.rs`: comment referencing `aida-gui`
- `build.rs`: comment referencing `aida-gui`
- `Trunk.toml`: comments referencing `aida-gui`

### 2f. Settings key — keep backward-compatible
- **Keep** `"aida_gui_settings"` as the settings key and `aida_gui_settings.yaml` as legacy path
- These are user data paths; renaming would break existing users' settings
- Add a comment explaining the legacy name

---

## Phase 3: Build infrastructure

### 3a. `Makefile`
- All `aida-gui` → `aida-desktop` in target names, paths, `cargo build -p`, `pkill`/`pgrep`, `cd aida-gui`, `trunk` commands (~18 occurrences)

### 3b. `.github/workflows/ci.yml`
- `gui_binary: aida-gui` → `gui_binary: aida-desktop` (lines 62, 66, 70)

### 3c. Docker files
- `docker/Dockerfile.web`: `COPY aida-gui` → `COPY aida-desktop`, `WORKDIR`, dist path
- `docker/Dockerfile.server`: `COPY aida-gui` → `COPY aida-desktop`

---

## Phase 4: Templates (embedded in binary)

### 4a. `aida-core/templates/skills/aida-req.md`
- `"aida-gui"` → `"aida-desktop"` (line 124)

### 4b. `aida-core/templates/skills/aida-implement.md`
- `"aida-gui"` → `"aida-desktop"` (line 23)

---

## Phase 5: Documentation

### 5a. Active docs (update all references):
- `OVERVIEW.md` — multiple references to `aida-gui` in structure, interfaces, features
- `CLAUDE.md` — if any direct references
- `docs/user-guide.md` — Desktop App section, binary references
- `docs/getting-started.md` — installation, launch instructions
- `docs/admin-guide.md` — if references exist
- `docs/DEVELOPER_GUIDE.md` — build instructions, architecture
- `README.md` — project structure, build commands
- `PLAN.md` — if referenced

### 5b. Historical docs (leave as-is):
- `PROMPT_HISTORY.md` — historical record, references are accurate for when they happened
- `docs/unified-gui-plan.md` — historical architectural plan
- `docs/unified-storage-architecture.md` — historical doc
- `docs/plans/*.md` — archived implementation plans
- `docs/AI_INTEGRATION_DESIGN.md` — historical design doc
- `requirements.yaml` / old YAML backups — historical data

### 5c. Regenerate HTML:
- Run `./helper/generate-docs.sh` to regenerate `user-guide.html` and `user-guide-dark.html`

---

## Implementation Order

1. `mv aida-gui/ aida-desktop/` (directory rename)
2. Cargo.toml files (root, aida-desktop, aida-web) + pnpm-workspace.yaml
3. Rust source: `aida_gui` → `aida_desktop` (main.rs, aida-web sources, internal strings)
4. Build infra: Makefile, CI, Docker
5. Templates: aida-core skills
6. Documentation: active docs only
7. Regenerate user-guide.html
8. `cargo build --workspace` to verify
9. Test: `aida-desktop` binary runs

---

## Verification

1. **`cargo build --workspace`** — full workspace compiles
2. **`./target/debug/aida-desktop`** — binary launches
3. **`./target/debug/aida-desktop --help`** — shows correct binary name
4. **`grep -r "aida-gui" --include="*.rs" --include="*.toml" --include="Makefile"`** — no stale references in build-critical files
5. **`grep -r "aida_gui" --include="*.rs"`** — only `aida_gui_settings` remains (intentional backward compat)
