// trace:STORY-3076 | ai:antigravity
//! Clean-room deterministic review preparation, path-scoped playbooks,
//! semantic file batching, file-coverage verification, and snippet-anchored
//! line positioning.
//!
//! This module provides deterministic engineering constraints for AIDA's
//! review pipeline so host reviewer agents operate over complete file
//! inventories, focused domain playbooks, token-bounded batches, and
//! verifiable line spans and observation citations.

use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Default token budget per review batch (~12,000 tokens, estimated at 4 bytes/token).
pub const DEFAULT_BATCH_TOKEN_BUDGET: usize = 12_000;

/// Default maximum files placed into a single review batch.
pub const DEFAULT_MAX_FILES_PER_BATCH: usize = 8;

/// Change status of a file in a git diff or workspace inventory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
    Scanned,
}

impl FileChangeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
            Self::Renamed => "renamed",
            Self::Untracked => "untracked",
            Self::Scanned => "scanned",
        }
    }
}

/// Raw entry describing one changed path discovered from git.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawDiffEntry {
    pub path: String,
    pub old_path: Option<String>,
    pub kind: FileChangeKind,
    pub additions: usize,
    pub deletions: usize,
    pub is_binary: bool,
    /// Approximate byte size of the diff patch or new file content.
    pub patch_bytes: usize,
}

/// Explicit reason why a changed path was excluded from active defect review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExclusionReason {
    BinaryFile,
    PureDeletion,
    GeneratedOrLockfile,
    AidaSubstrate,
    SecretSensitivePath,
    UserExcludedPattern(String),
}

impl ExclusionReason {
    pub fn label(&self) -> String {
        match self {
            Self::BinaryFile => "binary_file".to_string(),
            Self::PureDeletion => "pure_deletion".to_string(),
            Self::GeneratedOrLockfile => "generated_or_lockfile".to_string(),
            Self::AidaSubstrate => "aida_substrate".to_string(),
            Self::SecretSensitivePath => "secret_sensitive_path".to_string(),
            Self::UserExcludedPattern(p) => format!("user_excluded:{p}"),
        }
    }
}

/// A file selected for active code review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewableFile {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    pub kind: FileChangeKind,
    pub additions: usize,
    pub deletions: usize,
    pub estimated_tokens: usize,
    pub playbook_id: String,
    pub has_trace_comment: bool,
}

/// A file excluded from active code review along with its deterministic reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExcludedFile {
    pub path: String,
    pub kind: FileChangeKind,
    pub reason: String,
}

/// Complete classified inventory of a diff or workspace change set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassifiedInventory {
    pub reviewable_files: Vec<ReviewableFile>,
    pub excluded_files: Vec<ExcludedFile>,
    pub inventory_sha256: String,
}

/// Project-customizable review rules loaded from `.aida/review-rules.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectReviewRulesConfig {
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub batch_token_budget: Option<usize>,
    #[serde(default)]
    pub max_files_per_batch: Option<usize>,
    #[serde(default)]
    pub rules: Vec<CustomPathRule>,
}

/// A single path-scoped rule entry in `.aida/review-rules.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomPathRule {
    pub id: String,
    pub pattern: String,
    pub checklist: String,
    #[serde(default = "default_true")]
    pub merge_builtin: bool,
}

fn default_true() -> bool {
    true
}

impl ProjectReviewRulesConfig {
    /// Load `.aida/review-rules.toml` if present, otherwise return default config.
    pub fn load_from_root(project_root: &Path) -> Self {
        let path = project_root.join(".aida").join("review-rules.toml");
        if !path.is_file() {
            return Self::default();
        }
        match std::fs::read_to_string(&path) {
            Ok(raw) => toml::from_str(&raw).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }
}

/// Resolved review playbook for a path or group of paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedPlaybook {
    pub id: String,
    pub title: String,
    pub pattern: String,
    pub checklist: String,
    /// Optional diff-triggered conditional facet IDs activated by the target file contents.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active_facets: Vec<String>,
}

/// Conditional review facet appended to a language playbook only when the
/// reviewed file content or diff contains relevant constructs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaybookFacet {
    pub id: String,
    pub title: String,
    pub checklist: String,
}

/// Clean-room registry of built-in and project-defined path-scoped review playbooks.
#[derive(Debug, Clone)]
pub struct PlaybookRegistry {
    custom_rules: Vec<CustomPathRule>,
    builtin_rules: Vec<ResolvedPlaybook>,
    default_playbook: ResolvedPlaybook,
}

impl Default for PlaybookRegistry {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl PlaybookRegistry {
    /// Construct a registry with built-in Rust-first + polyglot review playbooks
    /// layered beneath optional project-specific rules.
    pub fn new(custom_rules: Vec<CustomPathRule>) -> Self {
        let builtin_rules = vec![
            ResolvedPlaybook {
                id: "rust".to_string(),
                title: "Rust Safety, Ownership, Error Handling & Traceability (Rust API / ANSSI / Tokio / Google Eng)".to_string(),
                pattern: "**/*.rs".to_string(),
                checklist: concat!(
                    "- **Anti-noise & evidence floor**: Do not duplicate diagnostics that `rustc`, the borrow checker, `cargo clippy`, or `rustfmt` catch reliably. Before flagging a non-local defect, verify call sites, trait bounds, and input boundaries.\n",
                    "- **Fail-closed error semantics (`fail-open`, `absent-evidence-reads-as-good`)**: Flag any `.unwrap()`, uncontextualized `.expect(\"\")`, or `panic!` in production paths. Flag `Result` or `Option` values silently mapped to empty/success defaults (`unwrap_or_default`, `if let Ok(...)`) where an I/O, git, or parse failure is misread as '0 items'.\n",
                    "- **Ownership, allocations & algorithmic complexity (`perf`)**: Flag `O(n^2)` nested scans where a `BTreeMap`/`HashMap`/`HashSet` or sorted index suffices, repeated `Regex::new` or collection rebuilds inside loops, and avoidable `.clone()`s where borrowing or `Cow` is cleaner.\n",
                    "- **UTF-8, integer casts & slice bounds (`portability`)**: Flag byte-index string slicing (`&s[a..b]`) not proven to lie on UTF-8 character boundaries, unchecked slice indexing (`arr[i]`), and lossy `as` numeric casts (`u64 as usize`, `usize as u32`) without overflow/truncation guards.\n",
                    "- **Caller contract & AIDA traceability (`contract-drift`, `incomplete-fix`, `untested-path`)**: When a signature or behavior changes, verify every call site and public CLI/MCP contract. Confirm non-trivial items carry `// trace:<SPEC-ID> | ai:<tool>` and tests falsify failure/edge paths, not just the happy path."
                ).to_string(),
                active_facets: Vec::new(),
            },
            ResolvedPlaybook {
                id: "cargo_toml".to_string(),
                title: "Cargo Manifest & Dependency Hygiene".to_string(),
                pattern: "**/Cargo.toml".to_string(),
                checklist: concat!(
                    "- **Feature gates**: Ensure optional native or network dependencies remain behind explicit Cargo features.\n",
                    "- **Workspace consistency**: Prefer `workspace = true` for shared dependencies and check for duplicate crate versions."
                ).to_string(),
                active_facets: Vec::new(),
            },
            ResolvedPlaybook {
                id: "go".to_string(),
                title: "Go Concurrency, Error Propagation & Resource Cleanup".to_string(),
                pattern: "**/*.go".to_string(),
                checklist: concat!(
                    "- **Error handling**: Check that returned errors are inspected and wrapped with context rather than silently discarded.\n",
                    "- **Resource lifecycle**: Verify `defer` cleanup on files, subprocesses, and HTTP response bodies.\n",
                    "- **Concurrency**: Check goroutine termination, `context.Context` cancellation propagation, and shared-map/slice synchronization."
                ).to_string(),
                active_facets: Vec::new(),
            },
            ResolvedPlaybook {
                id: "typescript_js".to_string(),
                title: "TypeScript & JavaScript Type Safety and Async Boundaries".to_string(),
                pattern: "**/*.{ts,tsx,js,jsx,mjs,cjs}".to_string(),
                checklist: concat!(
                    "- **Unhandled promises**: Verify `async` calls are awaited or have explicit error handling.\n",
                    "- **Injection & XSS**: Flag unsanitized shell interpolation, `eval`, or raw HTML injection.\n",
                    "- **Nullability & schema bounds**: Ensure external JSON payloads are validated before property access."
                ).to_string(),
                active_facets: Vec::new(),
            },
            ResolvedPlaybook {
                id: "python".to_string(),
                title: "Python Exception Safety, Subprocess Hygiene & Typing".to_string(),
                pattern: "**/*.py".to_string(),
                checklist: concat!(
                    "- **Subprocess & shell safety**: Flag `shell=True` with untrusted inputs and missing timeouts on external calls.\n",
                    "- **Exception handling**: Avoid bare `except:` blocks that swallow system or cancellation signals.\n",
                    "- **Mutable defaults**: Flag mutable default arguments (`def fn(items=[])`)."
                ).to_string(),
                active_facets: Vec::new(),
            },
            ResolvedPlaybook {
                id: "shell".to_string(),
                title: "Shell Script Quoting, Exit Traps & Portability".to_string(),
                pattern: "**/*.sh".to_string(),
                checklist: concat!(
                    "- **Strict mode**: Verify `set -euo pipefail` (or explicit error handling) and safe temporary file cleanup traps.\n",
                    "- **Variable quoting**: Ensure variable expansions and file paths are quoted to prevent word-splitting and glob injection."
                ).to_string(),
                active_facets: Vec::new(),
            },
            ResolvedPlaybook {
                id: "sql".to_string(),
                title: "SQL Parameterization, Transactions & Migration Safety".to_string(),
                pattern: "**/*.sql".to_string(),
                checklist: concat!(
                    "- **Parameterization**: Ensure queries use bound parameters rather than string concatenation.\n",
                    "- **Schema compatibility**: Verify migrations are idempotent or reversible and index foreign-key lookups."
                ).to_string(),
                active_facets: Vec::new(),
            },
            ResolvedPlaybook {
                id: "config_data".to_string(),
                title: "Configuration & Schema Integrity (TOML / YAML / JSON)".to_string(),
                pattern: "**/*.{toml,yaml,yml,json}".to_string(),
                checklist: concat!(
                    "- **Backward compatibility**: Verify renamed or removed config keys have serde aliases or migration fallbacks.\n",
                    "- **Secrets & environment**: Ensure no credentials, private tokens, or machine-local paths are hardcoded."
                ).to_string(),
                active_facets: Vec::new(),
            },
        ];

        let default_playbook = ResolvedPlaybook {
            id: "general".to_string(),
            title: "General Correctness, Security & AIDA Acceptance Review".to_string(),
            pattern: "**/*".to_string(),
            checklist: concat!(
                "- **Behavioral correctness**: Verify edge cases, boundary conditions, and backward compatibility for existing callers.\n",
                "- **Security**: Check for path traversal, command injection, and unintended authority bypasses.\n",
                "- **AIDA acceptance**: Confirm every linked requirement acceptance criterion touched by this file is satisfied."
            )
            .to_string(),
            active_facets: Vec::new(),
        };

        Self {
            custom_rules,
            builtin_rules,
            default_playbook,
        }
    }

    /// Resolve the effective base playbook for a relative repository path.
    pub fn resolve(&self, rel_path: &str) -> ResolvedPlaybook {
        let builtin = self
            .builtin_rules
            .iter()
            .find(|r| glob_matches(&r.pattern, rel_path))
            .unwrap_or(&self.default_playbook);

        for custom in &self.custom_rules {
            if glob_matches(&custom.pattern, rel_path) {
                if custom.merge_builtin {
                    return ResolvedPlaybook {
                        id: format!("{}+{}", builtin.id, custom.id),
                        title: format!("{} + Custom ({})", builtin.title, custom.id),
                        pattern: custom.pattern.clone(),
                        checklist: format!(
                            "{}\n\n### Project Rule ({})\n{}",
                            builtin.checklist,
                            custom.id,
                            custom.checklist.trim()
                        ),
                        active_facets: Vec::new(),
                    };
                } else {
                    return ResolvedPlaybook {
                        id: custom.id.clone(),
                        title: format!("Project Rule ({})", custom.id),
                        pattern: custom.pattern.clone(),
                        checklist: custom.checklist.trim().to_string(),
                        active_facets: Vec::new(),
                    };
                }
            }
        }

        builtin.clone()
    }

    /// Enrich a resolved playbook with zero-overhead conditional facets triggered
    /// only when the target file corpus contains matching constructs (e.g. `unsafe`,
    /// async/locks, subprocess/filesystem/serde boundaries, or macro definitions).
    // trace:STORY-3076 | ai:antigravity
    pub fn enrich_with_content_facets(
        &self,
        mut playbook: ResolvedPlaybook,
        corpus_samples: &[&str],
    ) -> ResolvedPlaybook {
        if !playbook.id.starts_with("rust") || corpus_samples.is_empty() {
            return playbook;
        }
        let facets = detect_rust_content_facets(corpus_samples);
        for facet in facets {
            playbook.active_facets.push(facet.id.clone());
            playbook.checklist.push_str(&format!(
                "\n\n#### Conditional Facet: {} (`{}`)\n{}",
                facet.title, facet.id, facet.checklist
            ));
        }
        playbook
    }
}

/// Inspect Rust source/diff samples and return only the specialized review facets
/// whose constructs actually appear in the code under review.
// trace:STORY-3076 | ai:antigravity
pub fn detect_rust_content_facets(samples: &[&str]) -> Vec<PlaybookFacet> {
    let has_any = |needles: &[&str]| -> bool {
        samples
            .iter()
            .any(|text| needles.iter().any(|n| text.contains(n)))
    };

    let mut facets = Vec::new();

    // 1. ANSSI / Rustonomicon Unsafe & FFI facet
    if has_any(&[
        "unsafe ",
        "unsafe{",
        "extern \"C\"",
        "MaybeUninit",
        "transmute",
        "from_raw_parts",
        "libc::",
    ]) {
        facets.push(PlaybookFacet {
            id: "rust_unsafe_ffi".to_string(),
            title: "Unsafe Blocks, Pointer Aliasing & FFI Boundaries (ANSSI / Rustonomicon)"
                .to_string(),
            checklist: concat!(
                "- **Minimality & proof**: Every `unsafe` block, `unsafe fn`, or `unsafe impl Send/Sync` must be as narrow as possible and carry an explicit `// SAFETY:` comment proving the invariant.\n",
                "- **Pointer & slice soundness**: Verify non-null, alignment, initialization, lifetime bounds, and absence of mutable aliasing (`&mut T` overlap) before `from_raw_parts` or raw pointer dereference.\n",
                "- **FFI & descriptor hygiene**: Check `libc` return codes (`-1` / `errno`), prevent Rust panics from unwinding across FFI boundaries, and verify file-descriptor / allocation ownership transfer."
            )
            .to_string(),
        });
    }

    // 2. Tokio / Concurrency & Lock-Safety facet
    if has_any(&[
        "async fn",
        ".await",
        "tokio::",
        "Mutex",
        "RwLock",
        "Atomic",
        "std::thread",
        "select!",
    ]) {
        facets.push(PlaybookFacet {
            id: "rust_async_concurrency".to_string(),
            title: "Async Cancellation, Lock Ordering & Shared State (`race` / Tokio Guidelines)"
                .to_string(),
            checklist: concat!(
                "- **Locks across `.await` & blocking in async**: Never hold a synchronous `std::sync::Mutex`/`RwLock` guard across `.await`, and do not run blocking filesystem/subprocess I/O on async worker threads.\n",
                "- **Cancellation safety**: In `tokio::select!` loops or cancellable futures, ensure dropping a future mid-flight cannot lose buffered data, leave partial writes, or leak a lock/permit.\n",
                "- **Lock ordering & atomics**: Verify consistent lock acquisition order across call paths, check-then-act TOCTOU races around shared caches/files, and justify `Atomic` memory orderings (`Ordering::Relaxed` vs `Acquire`/`Release`)."
            )
            .to_string(),
        });
    }

    // 3. ANSSI / Secure I/O, Subprocess & Deserialization facet
    if has_any(&[
        "Command::new",
        "std::fs::",
        "OpenOptions",
        "Path::new",
        "PathBuf",
        "serde_json::",
        "serde_yaml::",
        "toml::",
    ]) {
        facets.push(PlaybookFacet {
            id: "rust_io_subprocess_security".to_string(),
            title: "Subprocess Option Injection, Path Traversal & Deserialization Bounds (ANSSI)"
                .to_string(),
            checklist: concat!(
                "- **Subprocess option injection**: Never build commands via shell string interpolation; validate user/ref inputs with `reject_option_like` and pass `--end-of-options` (`END_OF_OPTIONS`) or `--` before positional refs/paths.\n",
                "- **Path traversal & TOCTOU**: Reject `Component::ParentDir` (`..`) and unexpected `RootDir` on external relative paths, verify symlink/containment boundaries, and check that revision-pinned reads never silently fall back to the live working tree.\n",
                "- **Schema & deserialization hygiene**: Bound untrusted input sizes before parsing, use `#[serde(deny_unknown_fields)]` on authority/verdict payloads, and preserve `#[serde(default)]` backward compatibility on config files."
            )
            .to_string(),
        });
    }

    // 4. Rust Macro Hygiene facet
    if has_any(&[
        "macro_rules!",
        "proc_macro",
        "proc_macro_attribute",
        "proc_macro_derive",
    ]) {
        facets.push(PlaybookFacet {
            id: "rust_macros".to_string(),
            title: "Macro Hygiene, Single Evaluation & Crate Resolution".to_string(),
            checklist: concat!(
                "- **Single evaluation**: Bind `$x:expr` arguments to a `let` once inside the macro expansion so caller side effects do not execute multiple times.\n",
                "- **Hygiene & precedence**: Prefix internal path references with `$crate::`, wrap re-emitted `$t:tt` fragments in parentheses, and emit `syn::Error`/`compile_error!` instead of panicking in proc macros."
            )
            .to_string(),
        });
    }

    facets
}

/// Clean-room glob matcher supporting `**`, `*`, `?`, and single-level `{a,b,c}` brace expansion.
/// Matching is case-insensitive on ASCII path characters and normalizes path separators to `/`.
// trace:STORY-3076 | ai:antigravity
pub fn glob_matches(pattern: &str, candidate: &str) -> bool {
    let norm_candidate = candidate.replace('\\', "/").to_ascii_lowercase();
    for expanded in expand_braces(pattern) {
        let norm_pat = expanded.replace('\\', "/").to_ascii_lowercase();
        let regex_str = glob_to_regex(&norm_pat);
        if let Ok(re) = Regex::new(&regex_str) {
            if re.is_match(&norm_candidate) {
                return true;
            }
        }
    }
    false
}

fn expand_braces(pattern: &str) -> Vec<String> {
    let Some(open) = pattern.find('{') else {
        return vec![pattern.to_string()];
    };
    let Some(rel_close) = pattern[open..].find('}') else {
        return vec![pattern.to_string()];
    };
    let close = open + rel_close;
    let prefix = &pattern[..open];
    let suffix = &pattern[close + 1..];
    pattern[open + 1..close]
        .split(',')
        .map(|alt| format!("{prefix}{}{suffix}", alt.trim()))
        .collect()
}

fn glob_to_regex(pat: &str) -> String {
    let mut out = String::from("^");
    let chars: Vec<char> = pat.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '*' && i + 1 < chars.len() && chars[i + 1] == '*' {
            if i + 2 < chars.len() && chars[i + 2] == '/' {
                out.push_str("(?:.*/)?");
                i += 3;
            } else {
                out.push_str(".*");
                i += 2;
            }
        } else if chars[i] == '*' {
            out.push_str("[^/]*");
            i += 1;
        } else if chars[i] == '?' {
            out.push_str("[^/]");
            i += 1;
        } else {
            let c = chars[i];
            if ".+^$()[]{}|\\".contains(c) {
                out.push('\\');
            }
            out.push(c);
            i += 1;
        }
    }
    out.push('$');
    out
}

/// Estimate review token weight from patch byte count and line churn.
pub fn estimate_file_tokens(patch_bytes: usize, additions: usize, deletions: usize) -> usize {
    let from_bytes = (patch_bytes + 3) / 4;
    let from_lines = (additions + deletions) * 12;
    from_bytes.max(from_lines).max(32)
}

/// Deterministic path exclusion check for generated, lockfile, substrate, and secret paths.
pub fn classify_exclusion(
    entry: &RawDiffEntry,
    user_excludes: &[String],
) -> Option<ExclusionReason> {
    if entry.is_binary {
        return Some(ExclusionReason::BinaryFile);
    }
    if entry.kind == FileChangeKind::Deleted {
        return Some(ExclusionReason::PureDeletion);
    }

    let norm = entry.path.replace('\\', "/");
    let file_name = Path::new(&norm)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&norm);

    // Check secret-sensitive paths first.
    if is_secret_sensitive_path(&norm, file_name) {
        return Some(ExclusionReason::SecretSensitivePath);
    }

    // Check AIDA runtime substrate paths.
    if norm.starts_with(".aida-store/")
        || norm == ".aida-store"
        || norm.starts_with(".aida/cache")
        || norm.starts_with(".aida/sessions/")
        || norm.starts_with(".aida/review-verdicts/")
        || norm.starts_with(".aida/locks/")
        || norm.starts_with(".aida/mailbox/")
    {
        return Some(ExclusionReason::AidaSubstrate);
    }

    // Check lockfiles and build outputs.
    if matches!(
        file_name,
        "Cargo.lock" | "package-lock.json" | "pnpm-lock.yaml" | "yarn.lock" | "go.sum"
    ) || norm.starts_with("target/")
        || norm.starts_with("node_modules/")
        || norm.starts_with("dist/")
        || norm.starts_with("vendor/")
        || norm.contains(".generated.")
    {
        return Some(ExclusionReason::GeneratedOrLockfile);
    }

    // Check user/project configured exclusion patterns.
    for pat in user_excludes {
        if glob_matches(pat, &norm) {
            return Some(ExclusionReason::UserExcludedPattern(pat.clone()));
        }
    }

    None
}

fn is_secret_sensitive_path(norm_path: &str, file_name: &str) -> bool {
    let lower_name = file_name.to_ascii_lowercase();
    if lower_name == ".env"
        || (lower_name.starts_with(".env.") && lower_name != ".env.example")
        || lower_name.ends_with(".pem")
        || lower_name.ends_with(".key")
        || lower_name.ends_with(".p12")
        || lower_name == "id_rsa"
        || lower_name == "id_ed25519"
        || lower_name == "credentials.json"
    {
        return true;
    }
    norm_path.contains("/.aws/") || norm_path.contains("/.ssh/")
}

/// Classify raw diff entries into reviewable and excluded lists, resolving each
/// reviewable file's playbook ID and computing a canonical SHA-256 inventory digest.
// trace:STORY-3076 | ai:antigravity
pub fn classify_diff_inventory<F>(
    entries: &[RawDiffEntry],
    registry: &PlaybookRegistry,
    user_excludes: &[String],
    mut trace_detector: F,
) -> ClassifiedInventory
where
    F: FnMut(&str) -> bool,
{
    let mut reviewable_files = Vec::new();
    let mut excluded_files = Vec::new();

    let mut sorted_entries = entries.to_vec();
    sorted_entries.sort_by(|a, b| a.path.cmp(&b.path));

    let mut hasher = Sha256::new();

    for entry in &sorted_entries {
        if let Some(reason) = classify_exclusion(entry, user_excludes) {
            let reason_str = reason.label();
            hasher.update(b"excluded:");
            hasher.update(entry.path.as_bytes());
            hasher.update(b":");
            hasher.update(reason_str.as_bytes());
            hasher.update(b"\n");
            excluded_files.push(ExcludedFile {
                path: entry.path.clone(),
                kind: entry.kind,
                reason: reason_str,
            });
        } else {
            let playbook = registry.resolve(&entry.path);
            let estimated_tokens =
                estimate_file_tokens(entry.patch_bytes, entry.additions, entry.deletions);
            let has_trace_comment = trace_detector(&entry.path);
            hasher.update(b"reviewable:");
            hasher.update(entry.path.as_bytes());
            hasher.update(b":");
            hasher.update(entry.kind.as_str().as_bytes());
            hasher.update(b":");
            hasher.update(playbook.id.as_bytes());
            hasher.update(b"\n");
            reviewable_files.push(ReviewableFile {
                path: entry.path.clone(),
                old_path: entry.old_path.clone(),
                kind: entry.kind,
                additions: entry.additions,
                deletions: entry.deletions,
                estimated_tokens,
                playbook_id: playbook.id,
                has_trace_comment,
            });
        }
    }

    let inventory_sha256 = format!("{:x}", hasher.finalize());
    ClassifiedInventory {
        reviewable_files,
        excluded_files,
        inventory_sha256,
    }
}

/// One token-bounded review batch produced by [`bundle_review_batches`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewBatch {
    pub batch_index: usize,
    pub label: String,
    pub playbook_ids: Vec<String>,
    pub files: Vec<String>,
    pub estimated_tokens: usize,
}

/// Normalize a file stem so implementation files and their sibling/companion test
/// files (`foo.rs` + `foo_test.rs` + `foo_tests.rs` + `foo.test.ts`) share a stem key.
/// Generic entry-point stems (`lib`, `mod`, `main`, `index`) are qualified with their
/// parent directory so unrelated crates (`aida-core/src/lib.rs` vs `aida-cli-lib/src/lib.rs`)
/// are not erroneously paired together.
pub fn canonical_module_stem(path: &str) -> String {
    let norm = path.replace('\\', "/");
    let file_name = Path::new(&norm)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&norm)
        .to_ascii_lowercase();

    let without_ext = match file_name.rfind('.') {
        Some(idx) if idx > 0 => &file_name[..idx],
        _ => &file_name,
    };

    let mut core_stem = without_ext;
    for suffix in ["_tests", "_test", ".test", ".spec"] {
        if let Some(stripped) = without_ext.strip_suffix(suffix) {
            if !stripped.is_empty() {
                core_stem = stripped;
                break;
            }
        }
    }
    if core_stem == without_ext {
        if let Some(stripped) = without_ext.strip_prefix("test_") {
            if !stripped.is_empty() {
                core_stem = stripped;
            }
        }
    }

    if matches!(core_stem, "lib" | "mod" | "main" | "index") {
        let parent_dir = Path::new(&norm)
            .parent()
            .map(|p| p.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if !parent_dir.is_empty() {
            return format!("{parent_dir}/{core_stem}");
        }
    }

    core_stem.to_string()
}

/// Bundle reviewable files into semantic, token-bounded batches:
/// 1. Files sharing a canonical module stem (e.g. `foo.rs` and `tests/foo_test.rs`)
///    are paired first so implementation and test changes are reviewed together.
/// 2. Unpaired files are grouped by shared `playbook_id`, packing into batches
///    that respect `max_tokens_per_batch` and `max_files_per_batch`.
// trace:STORY-3076 | ai:antigravity
pub fn bundle_review_batches(
    files: &[ReviewableFile],
    max_tokens_per_batch: usize,
    max_files_per_batch: usize,
) -> Vec<ReviewBatch> {
    if files.is_empty() {
        return Vec::new();
    }
    let token_cap = max_tokens_per_batch.max(256);
    let file_cap = max_files_per_batch.max(1);

    // Group by canonical module stem first to detect source+test companions.
    let mut by_stem: BTreeMap<String, Vec<&ReviewableFile>> = BTreeMap::new();
    for f in files {
        by_stem
            .entry(canonical_module_stem(&f.path))
            .or_default()
            .push(f);
    }

    let mut raw_batches: Vec<(String, Vec<&ReviewableFile>)> = Vec::new();
    let mut unpaired_by_playbook: BTreeMap<String, Vec<&ReviewableFile>> = BTreeMap::new();

    for (stem, group) in by_stem {
        if group.len() > 1 && group.len() <= file_cap {
            let sum_tokens: usize = group.iter().map(|f| f.estimated_tokens).sum();
            if sum_tokens <= token_cap {
                raw_batches.push((format!("module:{stem}"), group));
                continue;
            }
        }
        for f in group {
            unpaired_by_playbook
                .entry(f.playbook_id.clone())
                .or_default()
                .push(f);
        }
    }

    // Pack remaining files by playbook_id within token and file count budgets.
    for (playbook_id, group) in unpaired_by_playbook {
        let mut current: Vec<&ReviewableFile> = Vec::new();
        let mut current_tokens = 0usize;
        let mut part = 1usize;

        for f in group {
            let would_exceed_files = current.len() >= file_cap;
            let would_exceed_tokens =
                !current.is_empty() && current_tokens + f.estimated_tokens > token_cap;
            if would_exceed_files || would_exceed_tokens {
                raw_batches.push((
                    format!("playbook:{playbook_id}#{}", part),
                    std::mem::take(&mut current),
                ));
                current_tokens = 0;
                part += 1;
            }
            current_tokens += f.estimated_tokens;
            current.push(f);
        }
        if !current.is_empty() {
            let label = if part == 1 {
                format!("playbook:{playbook_id}")
            } else {
                format!("playbook:{playbook_id}#{}", part)
            };
            raw_batches.push((label, current));
        }
    }

    raw_batches
        .into_iter()
        .enumerate()
        .map(|(idx, (label, members))| {
            let mut playbooks: BTreeSet<String> = BTreeSet::new();
            let mut paths = Vec::with_capacity(members.len());
            let mut estimated_tokens = 0usize;
            for m in members {
                playbooks.insert(m.playbook_id.clone());
                paths.push(m.path.clone());
                estimated_tokens += m.estimated_tokens;
            }
            ReviewBatch {
                batch_index: idx + 1,
                label,
                playbook_ids: playbooks.into_iter().collect(),
                files: paths,
                estimated_tokens,
            }
        })
        .collect()
}

/// Disposition recorded by a reviewer for one file in the coverage ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileCoverageStatus {
    Reviewed,
    Skipped,
}

/// Entry in a reviewer's submitted coverage ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileCoverageEntry {
    pub path: String,
    pub status: FileCoverageStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Reviewer coverage ledger verified by [`verify_coverage_ledger`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageLedger {
    pub files: Vec<FileCoverageEntry>,
}

/// Result of deterministic file-coverage verification against a [`ClassifiedInventory`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageVerificationReport {
    pub passed: bool,
    pub total_reviewable: usize,
    pub reviewed_count: usize,
    pub skipped_count: usize,
    pub unaccounted_files: Vec<String>,
    pub skipped_without_reason: Vec<String>,
    pub coverage_rate_pct: usize,
}

/// Verify that every reviewable file in `inventory` is explicitly accounted for
/// in `ledger` (either `reviewed`, or `skipped` with a non-empty reason).
// trace:STORY-3076 | ai:antigravity
pub fn verify_coverage_ledger(
    inventory: &ClassifiedInventory,
    ledger: &CoverageLedger,
) -> CoverageVerificationReport {
    let mut by_path: BTreeMap<&str, &FileCoverageEntry> = BTreeMap::new();
    for entry in &ledger.files {
        by_path.insert(entry.path.as_str(), entry);
    }

    let mut reviewed_count = 0usize;
    let mut skipped_count = 0usize;
    let mut unaccounted_files = Vec::new();
    let mut skipped_without_reason = Vec::new();

    for req_file in &inventory.reviewable_files {
        match by_path.get(req_file.path.as_str()) {
            Some(entry) => match entry.status {
                FileCoverageStatus::Reviewed => {
                    reviewed_count += 1;
                }
                FileCoverageStatus::Skipped => {
                    skipped_count += 1;
                    let has_reason = entry
                        .reason
                        .as_deref()
                        .map(|r| !r.trim().is_empty())
                        .unwrap_or(false);
                    if !has_reason {
                        skipped_without_reason.push(req_file.path.clone());
                    }
                }
            },
            None => {
                unaccounted_files.push(req_file.path.clone());
            }
        }
    }

    let total_reviewable = inventory.reviewable_files.len();
    let accounted = reviewed_count + skipped_count.saturating_sub(skipped_without_reason.len());
    let coverage_rate_pct = if total_reviewable == 0 {
        100
    } else {
        (accounted * 100) / total_reviewable
    };

    let passed = unaccounted_files.is_empty() && skipped_without_reason.is_empty();

    CoverageVerificationReport {
        passed,
        total_reviewable,
        reviewed_count,
        skipped_count,
        unaccounted_files,
        skipped_without_reason,
        coverage_rate_pct,
    }
}

/// Resolved 1-based inclusive line span for a quoted code snippet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnchoredSpan {
    pub start_line: usize,
    pub end_line: usize,
    /// `"exact"` or `"trimmed_whitespace"`
    pub match_kind: String,
}

/// Deterministically resolve a quoted `existing_code` snippet to its 1-based
/// inclusive `(start_line, end_line)` in `file_content`.
///
/// If `hint_line` is provided and multiple matches exist, the match closest to
/// `hint_line` wins. First tries exact line matching, then falls back to
/// trimmed-whitespace matching (ignoring leading/trailing indentation drift).
// trace:STORY-3076 | ai:antigravity
pub fn anchor_snippet_lines(
    file_content: &str,
    snippet: &str,
    hint_line: Option<usize>,
) -> Option<AnchoredSpan> {
    let snippet_trimmed = snippet.trim_matches('\n').trim_matches('\r');
    if snippet_trimmed.trim().is_empty() {
        return None;
    }

    let file_lines: Vec<&str> = file_content.lines().collect();
    let needle_lines: Vec<&str> = snippet_trimmed.lines().collect();
    if needle_lines.is_empty() || needle_lines.len() > file_lines.len() {
        return None;
    }

    // Pass 1: exact line-by-line match.
    let mut exact_candidates = Vec::new();
    let window = needle_lines.len();
    for start_idx in 0..=(file_lines.len() - window) {
        let matches_exact = (0..window).all(|offset| {
            file_lines[start_idx + offset].trim_end_matches('\r')
                == needle_lines[offset].trim_end_matches('\r')
        });
        if matches_exact {
            exact_candidates.push((start_idx + 1, start_idx + window));
        }
    }

    if let Some((start_line, end_line)) = pick_closest_span(&exact_candidates, hint_line) {
        return Some(AnchoredSpan {
            start_line,
            end_line,
            match_kind: "exact".to_string(),
        });
    }

    // Pass 2: whitespace-normalized match (skipping leading/trailing whitespace differences).
    let norm_needle: Vec<&str> = needle_lines.iter().map(|l| l.trim()).collect();
    // Guard against matching a single blank or brace-only line everywhere.
    if norm_needle.iter().all(|l| l.is_empty()) {
        return None;
    }

    let mut trimmed_candidates = Vec::new();
    for start_idx in 0..=(file_lines.len() - window) {
        let matches_trimmed =
            (0..window).all(|offset| file_lines[start_idx + offset].trim() == norm_needle[offset]);
        if matches_trimmed {
            trimmed_candidates.push((start_idx + 1, start_idx + window));
        }
    }

    pick_closest_span(&trimmed_candidates, hint_line).map(|(start_line, end_line)| AnchoredSpan {
        start_line,
        end_line,
        match_kind: "trimmed_whitespace".to_string(),
    })
}

fn pick_closest_span(
    candidates: &[(usize, usize)],
    hint_line: Option<usize>,
) -> Option<(usize, usize)> {
    if candidates.is_empty() {
        return None;
    }
    let Some(hint) = hint_line else {
        return Some(candidates[0]);
    };
    candidates
        .iter()
        .copied()
        .min_by_key(|(start, _)| start.abs_diff(hint))
}

/// Recorded tool or diff observation during a review investigation (`[[obs-000001]]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewObservation {
    pub id: String,
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub content_sha256: String,
    #[serde(default)]
    pub has_error: bool,
}

/// Validation result for `[[obs-NNNNNN]]` citations inside a review report or verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CitationVerification {
    pub valid: bool,
    pub cited_ids: Vec<String>,
    pub unknown_ids: Vec<String>,
    pub errored_ids: Vec<String>,
}

/// Parse all `[[obs-...]]` markers in `markdown_text` and verify that each cited
/// observation exists in `observations` and did not record an error.
// trace:STORY-3076 | ai:antigravity
pub fn verify_observation_citations(
    markdown_text: &str,
    observations: &[ReviewObservation],
) -> CitationVerification {
    static OBS_RE: once_cell::sync::Lazy<Regex> = once_cell::sync::Lazy::new(|| {
        Regex::new(r"\[\[(obs-[A-Za-z0-9_-]+)\]\]").expect("static obs citation regex is valid")
    });
    let mut cited_set = BTreeSet::new();
    for cap in OBS_RE.captures_iter(markdown_text) {
        if let Some(m) = cap.get(1) {
            cited_set.insert(m.as_str().to_string());
        }
    }

    let obs_map: BTreeMap<&str, &ReviewObservation> =
        observations.iter().map(|o| (o.id.as_str(), o)).collect();

    let mut cited_ids = Vec::new();
    let mut unknown_ids = Vec::new();
    let mut errored_ids = Vec::new();

    for id in cited_set {
        cited_ids.push(id.clone());
        match obs_map.get(id.as_str()) {
            None => unknown_ids.push(id),
            Some(obs) if obs.has_error => errored_ids.push(id),
            Some(_) => {}
        }
    }

    let valid = unknown_ids.is_empty() && errored_ids.is_empty();
    CitationVerification {
        valid,
        cited_ids,
        unknown_ids,
        errored_ids,
    }
}

/// A review finding with an optional `existing_code` snippet to be line-anchored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawReviewFinding {
    pub path: String,
    pub severity: String,
    pub category: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub existing_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_status: Option<String>,
}

/// Anchor a batch of findings against a file-content provider, updating `start_line`,
/// `end_line`, and `anchor_status` (`"exact"`, `"trimmed_whitespace"`, `"unanchored"`, or `"no_snippet"`).
// trace:STORY-3076 | ai:antigravity
pub fn anchor_findings_with<F>(
    findings: &[RawReviewFinding],
    mut read_file: F,
) -> Vec<RawReviewFinding>
where
    F: FnMut(&str) -> Option<String>,
{
    let mut out = Vec::with_capacity(findings.len());
    for f in findings {
        let mut cloned = f.clone();
        if let Some(snippet) = f.existing_code.as_deref() {
            if let Some(content) = read_file(&f.path) {
                if let Some(span) = anchor_snippet_lines(&content, snippet, f.start_line) {
                    cloned.start_line = Some(span.start_line);
                    cloned.end_line = Some(span.end_line);
                    cloned.anchor_status = Some(span.match_kind);
                } else {
                    cloned.anchor_status = Some("unanchored".to_string());
                }
            } else {
                cloned.anchor_status = Some("file_unreadable".to_string());
            }
        } else {
            cloned.anchor_status = Some("no_snippet".to_string());
        }
        out.push(cloned);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_matches_with_braces_and_doublestar() {
        assert!(glob_matches("**/*.rs", "aida-core/src/lib.rs"));
        assert!(glob_matches("**/*.rs", "lib.rs"));
        assert!(glob_matches(
            "**/*.{ts,tsx,js}",
            "web/src/components/App.TSX"
        ));
        assert!(!glob_matches("**/*.{ts,tsx,js}", "web/src/styles.css"));
        assert!(glob_matches("**/Cargo.toml", "aida-core/Cargo.toml"));
        assert!(glob_matches("**/Cargo.toml", "Cargo.toml"));
    }

    #[test]
    fn test_classify_diff_inventory_and_exclusions() {
        let registry = PlaybookRegistry::default();
        let entries = vec![
            RawDiffEntry {
                path: "aida-core/src/review_engine.rs".to_string(),
                old_path: None,
                kind: FileChangeKind::Added,
                additions: 120,
                deletions: 0,
                is_binary: false,
                patch_bytes: 4800,
            },
            RawDiffEntry {
                path: "Cargo.lock".to_string(),
                old_path: None,
                kind: FileChangeKind::Modified,
                additions: 40,
                deletions: 10,
                is_binary: false,
                patch_bytes: 1600,
            },
            RawDiffEntry {
                path: ".env.production".to_string(),
                old_path: None,
                kind: FileChangeKind::Added,
                additions: 5,
                deletions: 0,
                is_binary: false,
                patch_bytes: 200,
            },
            RawDiffEntry {
                path: "old_deprecated.rs".to_string(),
                old_path: None,
                kind: FileChangeKind::Deleted,
                additions: 0,
                deletions: 80,
                is_binary: false,
                patch_bytes: 2000,
            },
            RawDiffEntry {
                path: "docs/legacy/notes.md".to_string(),
                old_path: None,
                kind: FileChangeKind::Modified,
                additions: 10,
                deletions: 2,
                is_binary: false,
                patch_bytes: 400,
            },
        ];

        let user_excludes = vec!["docs/legacy/**".to_string()];
        let inv = classify_diff_inventory(&entries, &registry, &user_excludes, |path| {
            path.ends_with("review_engine.rs")
        });

        assert_eq!(inv.reviewable_files.len(), 1);
        assert_eq!(
            inv.reviewable_files[0].path,
            "aida-core/src/review_engine.rs"
        );
        assert_eq!(inv.reviewable_files[0].playbook_id, "rust");
        assert!(inv.reviewable_files[0].has_trace_comment);

        assert_eq!(inv.excluded_files.len(), 4);
        let reasons: BTreeMap<String, String> = inv
            .excluded_files
            .iter()
            .map(|e| (e.path.clone(), e.reason.clone()))
            .collect();
        assert_eq!(
            reasons.get("Cargo.lock").map(String::as_str),
            Some("generated_or_lockfile")
        );
        assert_eq!(
            reasons.get(".env.production").map(String::as_str),
            Some("secret_sensitive_path")
        );
        assert_eq!(
            reasons.get("old_deprecated.rs").map(String::as_str),
            Some("pure_deletion")
        );
        assert_eq!(
            reasons.get("docs/legacy/notes.md").map(String::as_str),
            Some("user_excluded:docs/legacy/**")
        );
    }

    #[test]
    fn test_custom_playbook_merge_and_override() {
        let custom = vec![
            CustomPathRule {
                id: "gate_strict".to_string(),
                pattern: "**/gate/**/*.rs".to_string(),
                checklist: "- Verify gate authority never fails open.".to_string(),
                merge_builtin: true,
            },
            CustomPathRule {
                id: "proto_custom".to_string(),
                pattern: "**/*.proto".to_string(),
                checklist: "- Ensure field numbers are never reused.".to_string(),
                merge_builtin: false,
            },
        ];
        let reg = PlaybookRegistry::new(custom);

        let merged = reg.resolve("aida-cli-lib/src/gate/review_verdict.rs");
        assert_eq!(merged.id, "rust+gate_strict");
        assert!(merged.checklist.contains("Flag any `.unwrap()`"));
        assert!(merged
            .checklist
            .contains("Verify gate authority never fails open."));

        let replaced = reg.resolve("proto/aida.proto");
        assert_eq!(replaced.id, "proto_custom");
        assert_eq!(
            replaced.checklist,
            "- Ensure field numbers are never reused."
        );
    }

    #[test]
    fn test_diff_triggered_rust_content_facets_zero_overhead_when_absent() {
        let reg = PlaybookRegistry::default();
        let base = reg.resolve("src/pure_math.rs");

        // Pure arithmetic file triggers zero conditional facets
        let pure_enriched = reg.enrich_with_content_facets(
            base.clone(),
            &["pub fn add(a: i32, b: i32) -> i32 { a + b }"],
        );
        assert!(pure_enriched.active_facets.is_empty());
        assert!(!pure_enriched.checklist.contains("Conditional Facet:"));

        // File using Command::new and unsafe triggers only io_subprocess_security and unsafe_ffi
        let io_unsafe_sample = "\
            use std::process::Command;
            pub fn run() {
                let _ = Command::new(\"git\").status();
                unsafe { libc::getpid(); }
            }
        ";
        let enriched = reg.enrich_with_content_facets(base, &[io_unsafe_sample]);
        assert_eq!(
            enriched.active_facets,
            vec![
                "rust_unsafe_ffi".to_string(),
                "rust_io_subprocess_security".to_string()
            ]
        );
        assert!(enriched.checklist.contains("rust_unsafe_ffi"));
        assert!(enriched.checklist.contains("rust_io_subprocess_security"));
        assert!(!enriched.checklist.contains("rust_macros"));
        assert!(!enriched.checklist.contains("rust_async_concurrency"));
    }

    #[test]
    fn test_bundle_review_batches_pairs_tests_and_respects_budget() {
        let files = vec![
            ReviewableFile {
                path: "src/parser.rs".to_string(),
                old_path: None,
                kind: FileChangeKind::Modified,
                additions: 50,
                deletions: 10,
                estimated_tokens: 800,
                playbook_id: "rust".to_string(),
                has_trace_comment: true,
            },
            ReviewableFile {
                path: "tests/parser_test.rs".to_string(),
                old_path: None,
                kind: FileChangeKind::Modified,
                additions: 40,
                deletions: 0,
                estimated_tokens: 600,
                playbook_id: "rust".to_string(),
                has_trace_comment: true,
            },
            ReviewableFile {
                path: "src/alpha.rs".to_string(),
                old_path: None,
                kind: FileChangeKind::Modified,
                additions: 100,
                deletions: 20,
                estimated_tokens: 1500,
                playbook_id: "rust".to_string(),
                has_trace_comment: true,
            },
            ReviewableFile {
                path: "src/beta.rs".to_string(),
                old_path: None,
                kind: FileChangeKind::Modified,
                additions: 100,
                deletions: 20,
                estimated_tokens: 1500,
                playbook_id: "rust".to_string(),
                has_trace_comment: true,
            },
        ];

        // Budget 2000 tokens: parser.rs + parser_test.rs (1400 tokens) pair into `module:parser`,
        // while alpha.rs (1500) and beta.rs (1500) split into two separate playbook batches.
        let batches = bundle_review_batches(&files, 2000, 4);
        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0].label, "module:parser");
        assert_eq!(
            batches[0].files,
            vec![
                "src/parser.rs".to_string(),
                "tests/parser_test.rs".to_string()
            ]
        );
        assert_eq!(batches[0].estimated_tokens, 1400);
        assert_eq!(batches[1].files, vec!["src/alpha.rs".to_string()]);
        assert_eq!(batches[2].files, vec!["src/beta.rs".to_string()]);

        // Verify generic stems (`lib.rs`, `mod.rs`) across different crates do not collide
        assert_ne!(
            canonical_module_stem("aida-core/src/lib.rs"),
            canonical_module_stem("aida-cli-lib/src/lib.rs")
        );
    }

    #[test]
    fn test_verify_coverage_ledger_enforces_100_percent_accountability() {
        let inv = ClassifiedInventory {
            reviewable_files: vec![
                ReviewableFile {
                    path: "src/a.rs".to_string(),
                    old_path: None,
                    kind: FileChangeKind::Modified,
                    additions: 10,
                    deletions: 2,
                    estimated_tokens: 150,
                    playbook_id: "rust".to_string(),
                    has_trace_comment: true,
                },
                ReviewableFile {
                    path: "src/b.rs".to_string(),
                    old_path: None,
                    kind: FileChangeKind::Modified,
                    additions: 20,
                    deletions: 5,
                    estimated_tokens: 300,
                    playbook_id: "rust".to_string(),
                    has_trace_comment: true,
                },
            ],
            excluded_files: Vec::new(),
            inventory_sha256: "abc".to_string(),
        };

        let incomplete_ledger = CoverageLedger {
            files: vec![
                FileCoverageEntry {
                    path: "src/a.rs".to_string(),
                    status: FileCoverageStatus::Reviewed,
                    reason: None,
                },
                FileCoverageEntry {
                    path: "src/b.rs".to_string(),
                    status: FileCoverageStatus::Skipped,
                    reason: Some("   ".to_string()),
                },
            ],
        };

        let rep = verify_coverage_ledger(&inv, &incomplete_ledger);
        assert!(!rep.passed);
        assert_eq!(rep.skipped_without_reason, vec!["src/b.rs".to_string()]);
        assert_eq!(rep.coverage_rate_pct, 50);

        let complete_ledger = CoverageLedger {
            files: vec![
                FileCoverageEntry {
                    path: "src/a.rs".to_string(),
                    status: FileCoverageStatus::Reviewed,
                    reason: None,
                },
                FileCoverageEntry {
                    path: "src/b.rs".to_string(),
                    status: FileCoverageStatus::Skipped,
                    reason: Some("Fixture-only rename validated by a.rs".to_string()),
                },
            ],
        };
        let rep_ok = verify_coverage_ledger(&inv, &complete_ledger);
        assert!(rep_ok.passed);
        assert_eq!(rep_ok.coverage_rate_pct, 100);
    }

    #[test]
    fn test_anchor_snippet_lines_exact_and_whitespace_tolerant() {
        let source = "\
fn alpha() {
    let x = 1;
}

fn beta() {
    if condition() {
        let danger = compute().unwrap();
        process(danger);
    }
}
";
        // Exact match
        let span = anchor_snippet_lines(
            source,
            "        let danger = compute().unwrap();\n        process(danger);",
            None,
        )
        .expect("should anchor exact snippet");
        assert_eq!(span.start_line, 7);
        assert_eq!(span.end_line, 8);
        assert_eq!(span.match_kind, "exact");

        // Whitespace-drifted snippet from LLM
        let drifted = anchor_snippet_lines(
            source,
            "let danger = compute().unwrap();\nprocess(danger);",
            Some(5),
        )
        .expect("should anchor whitespace-drifted snippet");
        assert_eq!(drifted.start_line, 7);
        assert_eq!(drifted.end_line, 8);
        assert_eq!(drifted.match_kind, "trimmed_whitespace");
    }

    #[test]
    fn test_verify_observation_citations() {
        let observations = vec![
            ReviewObservation {
                id: "obs-000001".to_string(),
                path: "src/lib.rs".to_string(),
                start_line: 10,
                end_line: 25,
                content_sha256: "deadbeef".to_string(),
                has_error: false,
            },
            ReviewObservation {
                id: "obs-000002".to_string(),
                path: "src/missing.rs".to_string(),
                start_line: 0,
                end_line: 0,
                content_sha256: "".to_string(),
                has_error: true,
            },
        ];

        let good_report = "Verified callers in `src/lib.rs` [[obs-000001]].";
        let res_ok = verify_observation_citations(good_report, &observations);
        assert!(res_ok.valid);
        assert_eq!(res_ok.cited_ids, vec!["obs-000001".to_string()]);

        let bad_report =
            "Checked [[obs-000001]], errored [[obs-000002]], and hallucinated [[obs-999999]].";
        let res_bad = verify_observation_citations(bad_report, &observations);
        assert!(!res_bad.valid);
        assert_eq!(res_bad.unknown_ids, vec!["obs-999999".to_string()]);
        assert_eq!(res_bad.errored_ids, vec!["obs-000002".to_string()]);
    }
}
