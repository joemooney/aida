// trace:STORY-3076 | ai:antigravity
//! Clean-room deterministic review preparation, path-scoped playbooks,
//! semantic file batching, file-coverage verification, and snippet-anchored
//! line positioning.
//!
//! This module provides deterministic engineering constraints for AIDA's
//! review pipeline so host reviewer agents operate over complete file
//! inventories, focused domain playbooks, token-bounded batches, and
//! verifiable line spans and observation citations.

use anyhow::Context;
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
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
    /// Optional project-level default agent for `aida review prepare --run-review`
    /// (`claude`, `codex`, `antigravity`/`agy`, or `gemini`).
    // trace:STORY-3120 | ai:antigravity
    #[serde(default)]
    pub default_agent: Option<String>,
    /// Optional project-level default model for `aida review prepare --run-review`.
    // trace:STORY-3120 | ai:antigravity
    #[serde(default)]
    pub default_model: Option<String>,
    /// Optional project-level default reasoning effort (`low`, `medium`, `high`, `xhigh`)
    /// for `aida review prepare --run-review`.
    // trace:STORY-3120 | ai:antigravity
    #[serde(default)]
    pub default_effort: Option<String>,
    /// Optional project-level default enabling two-pass Red-Team Finder -> Blue-Team Challenger review.
    // trace:STORY-3120 | ai:antigravity
    #[serde(default)]
    pub adversarial: Option<bool>,
    /// Optional project-level default challenger agent for Pass 2 of `--adversarial`.
    // trace:STORY-3120 | ai:antigravity
    #[serde(default)]
    pub challenger_agent: Option<String>,
    /// Optional project-level default challenger model for Pass 2 of `--adversarial`.
    // trace:STORY-3120 | ai:antigravity
    #[serde(default)]
    pub challenger_model: Option<String>,
    /// Optional project-level default challenger reasoning effort (`low`, `medium`, `high`, `xhigh`)
    /// for Pass 2 of `--adversarial`.
    // trace:STORY-3120 | ai:antigravity
    #[serde(default)]
    pub challenger_effort: Option<String>,
    /// Optional project-level severity threshold (`critical`, `high`, `medium`, `low`)
    /// at or above which `aida review prepare` exits non-zero as a CI quality gate.
    // trace:STORY-3120 | ai:antigravity
    #[serde(default)]
    pub fail_on: Option<String>,
    /// Optional project-level toggle to persist each review's SARIF 2.1.0 archive and
    /// telemetry row under `.aida/review-telemetry/`.
    // trace:STORY-3120 | ai:antigravity
    #[serde(default)]
    pub record_telemetry: Option<bool>,
    /// Optional project-level A/B experiment arm label (`single-pass`, `adversarial`,
    /// or `auto` for deterministic 50/50 randomization by PR number / commit SHA).
    // trace:STORY-3120 | ai:antigravity
    #[serde(default)]
    pub ab_arm: Option<String>,
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
    // trace:TASK-3866 | ai:codex
    pub fn load_from_root(project_root: &Path) -> anyhow::Result<Self> {
        let path = project_root.join(".aida").join("review-rules.toml");
        let mut file = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path)
        {
            Err(err)
                if err.kind() == std::io::ErrorKind::NotFound
                    && matches!(std::fs::symlink_metadata(&path), Err(e) if e.kind() == std::io::ErrorKind::NotFound) =>
            {
                return Ok(Self::default())
            }
            result => result.with_context(|| format!("open {}", path.display()))?,
        };
        let metadata = file
            .metadata()
            .with_context(|| format!("inspect {}", path.display()))?;
        anyhow::ensure!(
            metadata.is_file(),
            "{} is not a regular file",
            path.display()
        );
        let mut raw = String::new();
        file.read_to_string(&mut raw)
            .with_context(|| format!("read {}", path.display()))?;
        toml::from_str(&raw).with_context(|| format!("parse {}", path.display()))
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
                id: "c_cpp".to_string(),
                title: "C & C++ Memory Lifetime, RAII, Buffer/Integer UB & Thread Safety (CERT / Core Guidelines)".to_string(),
                pattern: "**/*.{c,cc,cpp,cxx,h,hh,hpp,hxx}".to_string(),
                checklist: concat!(
                    "- **Memory lifetime & RAII**: Flag raw owning pointers, missing destructors/cleanup on error paths, use-after-free, double-free, and dangling references/iterators or `std::string_view`/`std::span` outliving their backing storage.\n",
                    "- **Buffer bounds & integer UB**: Flag unchecked array indexing, `memcpy`/`strcpy`/`sprintf` without bounds checks, signed integer overflow, shift-by-width UB, and uninitialized stack/struct fields.\n",
                    "- **Concurrency & header contracts**: Verify mutex lock ordering, atomic memory ordering, `errno`/return-code checks on OS/POSIX calls, and header/implementation signature and `noexcept` parity."
                ).to_string(),
                active_facets: Vec::new(),
            },
            ResolvedPlaybook {
                id: "java".to_string(),
                title: "Java Resource Lifecycle, Concurrency, Nullability & Injection Safety".to_string(),
                pattern: "**/*.java".to_string(),
                checklist: concat!(
                    "- **Resource lifecycle (`try-with-resources`)**: Ensure `AutoCloseable` streams, JDBC connections, statements, and sockets are closed via `try-with-resources` on all paths.\n",
                    "- **Concurrency & immutability**: Flag unsynchronized mutation of shared collections/fields across threads, lock acquisition while calling external code, and broken `equals`/`hashCode` contracts.\n",
                    "- **Nullability, exceptions & injection**: Flag swallowed exceptions, unchecked null dereferences across API boundaries, and unparameterized SQL, LDAP, command, or deserialization inputs."
                ).to_string(),
                active_facets: Vec::new(),
            },
            ResolvedPlaybook {
                id: "matlab".to_string(),
                title: "MATLAB Matrix Dimensions, Vectorization, Numerical Conditioning & Scope Hygiene".to_string(),
                pattern: "**/*.m".to_string(),
                checklist: concat!(
                    "- **Dimensions, 1-based indexing & broadcasting**: Verify row-vs-column vector orientation, unintended implicit expansion (`bsxfun`/broadcasting) on mismatched dimensions, and off-by-one 1-based indexing bounds.\n",
                    "- **Numerical stability & conditioning**: Flag explicit matrix inversion (`inv(A)*b` instead of `A\\b`), unguarded division by near-zero singular values, and silent `NaN`/`Inf` propagation.\n",
                    "- **Preallocation & workspace hygiene**: Flag arrays grown dynamically inside loops without preallocation (`zeros`/`nan`), `eval`/`feval` on dynamic strings, and variables shadowing built-in MATLAB functions."
                ).to_string(),
                active_facets: Vec::new(),
            },
            ResolvedPlaybook {
                id: "hdl_rtl".to_string(),
                title: "Verilog, SystemVerilog & VHDL RTL Synthesis, CDC, Reset & Latch Safety".to_string(),
                pattern: "**/*.{v,vh,sv,svh,vhd,vhdl}".to_string(),
                checklist: concat!(
                    "- **Inferred latches & combinational completeness**: Flag combinational `always @(*)` / `always_comb` / `process` blocks with incomplete `if`/`else` or `case` statements lacking default assignments on all outputs.\n",
                    "- **Blocking vs. non-blocking & multiple drivers**: Verify sequential clocked logic uses non-blocking assignments (`<=`) while combinational logic uses blocking assignments (`=`), and flag signals driven from multiple `always`/process blocks.\n",
                    "- **Clock-domain crossing (CDC), resets & bit widths**: Flag single-bit or multi-bit signals crossing asynchronous clock domains without proper synchronizers/FIFOs, incomplete reset initialization, and silent bit-width truncation or signed/unsigned mismatch."
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
    let from_bytes = patch_bytes.div_ceil(4);
    let from_lines = additions.saturating_add(deletions).saturating_mul(12);
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

    // trace:TASK-3866 | ai:codex
    std::iter::once(entry.path.as_str())
        .chain(entry.old_path.as_deref())
        .find_map(|path| classify_path_exclusion(path, user_excludes))
}

fn classify_path_exclusion(path: &str, user_excludes: &[String]) -> Option<ExclusionReason> {
    let norm = path.replace('\\', "/");
    let file_name = Path::new(&norm)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&norm);

    // Check secret-sensitive paths first.
    if is_secret_sensitive_path(&norm, file_name) {
        return Some(ExclusionReason::SecretSensitivePath);
    }

    // Check AIDA runtime substrate paths.
    let path = Path::new(&norm);
    if [
        ".aida-store",
        ".aida/cache",
        ".aida/cache-db",
        ".aida/sessions",
        ".aida/review-verdicts",
        ".aida/locks",
        ".aida/mailbox",
    ]
    .iter()
    .any(|prefix| path.starts_with(prefix))
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
    norm_path
        .split('/')
        .any(|part| matches!(part, ".aws" | ".ssh"))
}

/// Classify raw diff entries into reviewable and excluded lists, resolving each
/// reviewable file's playbook ID and computing a canonical SHA-256 inventory digest.
// trace:STORY-3076 | ai:antigravity
pub fn classify_diff_inventory<F>(
    entries: &[RawDiffEntry],
    registry: &PlaybookRegistry,
    user_excludes: &[String],
    token_cap: usize,
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

    // trace:TASK-3867 | ai:codex
    let mut frame = |fields: &[&str]| {
        for field in fields {
            hasher.update((field.len() as u64).to_be_bytes());
            hasher.update(field.as_bytes());
        }
    };
    for entry in &sorted_entries {
        let estimated_tokens =
            estimate_file_tokens(entry.patch_bytes, entry.additions, entry.deletions);
        let reason = classify_exclusion(entry, user_excludes)
            .map(|r| r.label())
            .or_else(|| {
                (estimated_tokens > token_cap).then(|| {
                    format!("oversize: {estimated_tokens} estimated tokens > cap {token_cap}")
                })
            });
        if let Some(reason_str) = reason {
            frame(&["excluded", &entry.path, &reason_str]);
            excluded_files.push(ExcludedFile {
                path: entry.path.clone(),
                kind: entry.kind,
                reason: reason_str,
            });
        } else {
            let playbook = registry.resolve(&entry.path);
            let has_trace_comment = trace_detector(&entry.path);
            frame(&["reviewable", &entry.path, entry.kind.as_str(), &playbook.id]);
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
    let raw_file_name = Path::new(&norm)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&norm);
    let raw_without_ext = match raw_file_name.rfind('.') {
        Some(idx) if idx > 0 => &raw_file_name[..idx],
        _ => raw_file_name,
    };
    // Strip camelCase Java/Kotlin test suffixes (`FooTest` -> `Foo`, `FooTests` -> `Foo`)
    // before lowercasing so lowercase words ending in "test" (like `contest.rs`) are preserved.
    let camel_stripped = raw_without_ext
        .strip_suffix("Tests")
        .or_else(|| raw_without_ext.strip_suffix("Test"))
        .filter(|prefix| {
            !prefix.is_empty()
                && prefix
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_uppercase())
        })
        .unwrap_or(raw_without_ext);

    let without_ext_owned = camel_stripped.to_ascii_lowercase();
    let without_ext = without_ext_owned.as_str();

    let mut core_stem = without_ext;
    for suffix in ["_tests", "_test", ".test", ".spec", "_tb"] {
        if let Some(stripped) = without_ext.strip_suffix(suffix) {
            if !stripped.is_empty() {
                core_stem = stripped;
                break;
            }
        }
    }
    if core_stem == without_ext {
        for prefix in ["test_", "tb_"] {
            if let Some(stripped) = without_ext.strip_prefix(prefix) {
                if !stripped.is_empty() {
                    core_stem = stripped;
                    break;
                }
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
/// Returns an error when a single file exceeds the token budget.
// trace:STORY-3076 | ai:antigravity
pub fn bundle_review_batches(
    files: &[ReviewableFile],
    max_tokens_per_batch: usize,
    max_files_per_batch: usize,
) -> anyhow::Result<Vec<ReviewBatch>> {
    let token_cap = max_tokens_per_batch;
    if let Some(file) = files.iter().find(|f| f.estimated_tokens > token_cap) {
        anyhow::bail!(
            "{} requires {} tokens, exceeding batch budget {token_cap}",
            file.path,
            file.estimated_tokens
        );
    }
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
            let sum_tokens = group
                .iter()
                .try_fold(0usize, |sum, f| sum.checked_add(f.estimated_tokens));
            if sum_tokens.is_some_and(|sum| sum <= token_cap) {
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
            let would_exceed_tokens = !current.is_empty()
                && f.estimated_tokens > token_cap.saturating_sub(current_tokens);
            if would_exceed_files || would_exceed_tokens {
                raw_batches.push((
                    format!("playbook:{playbook_id}#{}", part),
                    std::mem::take(&mut current),
                ));
                current_tokens = 0;
                part += 1;
            }
            current_tokens = current_tokens.saturating_add(f.estimated_tokens);
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

    Ok(raw_batches
        .into_iter()
        .enumerate()
        .map(|(idx, (label, members))| {
            let mut playbooks: BTreeSet<String> = BTreeSet::new();
            let mut paths = Vec::with_capacity(members.len());
            let mut estimated_tokens = 0usize;
            for m in members {
                playbooks.insert(m.playbook_id.clone());
                paths.push(m.path.clone());
                estimated_tokens = estimated_tokens.saturating_add(m.estimated_tokens);
            }
            ReviewBatch {
                batch_index: idx + 1,
                label,
                playbook_ids: playbooks.into_iter().collect(),
                files: paths,
                estimated_tokens,
            }
        })
        .collect())
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
    #[serde(default)]
    pub not_reviewable_by_batch: Vec<ExcludedFile>,
    pub total_reviewable: usize,
    pub reviewed_count: usize,
    pub skipped_count: usize,
    #[serde(default)]
    pub ambiguous_files: Vec<String>,
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
    let mut ambiguous = BTreeSet::new();
    for entry in &ledger.files {
        if by_path.insert(entry.path.as_str(), entry).is_some() {
            ambiguous.insert(entry.path.clone());
        }
    }

    let mut reviewed_count = 0usize;
    let mut skipped_count = 0usize;
    let mut unaccounted_files = Vec::new();
    let mut skipped_without_reason = Vec::new();

    for req_file in &inventory.reviewable_files {
        if ambiguous.contains(&req_file.path) {
            continue;
        }
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

    let passed =
        ambiguous.is_empty() && unaccounted_files.is_empty() && skipped_without_reason.is_empty();

    CoverageVerificationReport {
        passed,
        not_reviewable_by_batch: inventory
            .excluded_files
            .iter()
            .filter(|f| f.reason.starts_with("oversize: "))
            .cloned()
            .collect(),
        total_reviewable,
        reviewed_count,
        skipped_count,
        ambiguous_files: ambiguous.into_iter().collect(),
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

/// Validation result for observation citations inside a review report or verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CitationVerification {
    pub valid: bool,
    pub cited_ids: Vec<String>,
    pub unknown_ids: Vec<String>,
    #[serde(default)]
    pub ambiguous_ids: Vec<String>,
    pub errored_ids: Vec<String>,
}

/// Parse single- and double-bracket observation markers in `markdown_text` and verify that each cited
/// observation exists in `observations` and did not record an error.
// trace:STORY-3076 | ai:antigravity
pub fn verify_observation_citations(
    markdown_text: &str,
    observations: &[ReviewObservation],
) -> CitationVerification {
    static OBS_RE: once_cell::sync::Lazy<Regex> = once_cell::sync::Lazy::new(|| {
        Regex::new(r"\[\[(obs-[A-Za-z0-9_-]+)\]\]|\[(obs-[A-Za-z0-9_-]+)\]")
            .expect("static obs citation regex is valid")
    });
    let mut cited_set = BTreeSet::new();
    for cap in OBS_RE.captures_iter(markdown_text) {
        if let Some(m) = cap.get(1).or_else(|| cap.get(2)) {
            cited_set.insert(m.as_str().to_string());
        }
    }

    let mut obs_map = BTreeMap::new();
    let mut ambiguous = BTreeSet::new();
    for obs in observations {
        if obs_map.insert(obs.id.as_str(), obs).is_some() {
            ambiguous.insert(obs.id.clone());
        }
    }

    let mut cited_ids = Vec::new();
    let mut unknown_ids = Vec::new();
    let mut errored_ids = Vec::new();

    for id in cited_set {
        cited_ids.push(id.clone());
        if ambiguous.contains(&id) {
            continue;
        }
        match obs_map.get(id.as_str()) {
            None => unknown_ids.push(id),
            Some(obs) if obs.has_error => errored_ids.push(id),
            Some(_) => {}
        }
    }

    let valid = ambiguous.is_empty() && unknown_ids.is_empty() && errored_ids.is_empty();
    CitationVerification {
        valid,
        cited_ids,
        unknown_ids,
        ambiguous_ids: ambiguous.into_iter().collect(),
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

/// A Pass-1 candidate finding that was dismissed as a false positive by the Pass-2 Blue-Team Challenger.
/// Retained in `AdversarialReviewSummary` and exported into SARIF 2.1.0 `results` with a standard
/// SARIF §3.35 `suppressions` entry (`kind: "external"`, `status: "accepted"`, `justification`).
// trace:STORY-3120 | ai:antigravity
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DismissedReviewFinding {
    pub finding: RawReviewFinding,
    pub dismissal_reason: String,
}

/// Comprehensive telemetry record for a single `aida review prepare` execution.
/// Stored in `.aida/review-telemetry/reviews.jsonl` (and embedded in SARIF 2.1.0
/// `runs[0].properties.aidaTelemetry`) for A/B testing, latency/cost analysis,
/// and false-positive tracking.
// trace:STORY-3120 | ai:antigravity
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewTelemetryRecord {
    pub review_id: String,
    pub timestamp: String,
    /// A/B experiment arm (`"prepare-only"`, `"single-pass"`, `"adversarial"`, `"ab-single-pass"`, `"ab-adversarial"`, or custom `--ab-arm`).
    pub ab_arm: String,
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_number: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_ref: Option<String>,
    pub inventory_sha256: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub linked_specs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detected_author_agents: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finder_agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finder_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finder_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub challenger_agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub challenger_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub challenger_effort: Option<String>,
    /// Diff scope metrics
    pub reviewable_files: usize,
    pub excluded_files: usize,
    pub batch_count: usize,
    pub total_added_lines: usize,
    pub total_removed_lines: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active_playbooks: Vec<String>,
    /// Wall-clock duration breakdown in milliseconds
    pub prepare_elapsed_ms: u64,
    pub finder_elapsed_ms: u64,
    pub challenger_elapsed_ms: u64,
    pub verify_elapsed_ms: u64,
    pub total_elapsed_ms: u64,
    /// Prompt/response size, token estimates, and estimated cost in USD
    pub finder_prompt_bytes: usize,
    pub finder_response_bytes: usize,
    pub challenger_prompt_bytes: usize,
    pub challenger_response_bytes: usize,
    pub estimated_input_tokens: usize,
    pub estimated_output_tokens: usize,
    pub estimated_cost_usd: f64,
    /// Finding quality, signal-to-noise, and anchor precision metrics
    pub pass1_candidate_findings: usize,
    pub confirmed_findings: usize,
    pub dismissed_findings: usize,
    pub false_positive_suppression_pct: u8,
    pub critical_count: usize,
    pub high_count: usize,
    pub medium_count: usize,
    pub low_count: usize,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub counts_by_category: BTreeMap<String, usize>,
    pub exact_anchor_count: usize,
    pub shifted_anchor_count: usize,
    pub unanchored_count: usize,
    pub exact_anchor_rate_pct: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage_rate_pct: Option<u8>,
    pub gate_failed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sarif_archive_path: Option<String>,
}

/// Aggregated statistics for a single A/B experiment arm across recorded reviews.
// trace:STORY-3120 | ai:antigravity
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewArmSummary {
    pub ab_arm: String,
    pub runs: usize,
    pub avg_total_ms: u64,
    pub avg_prepare_ms: u64,
    pub avg_finder_ms: u64,
    pub avg_challenger_ms: u64,
    pub avg_input_tokens: usize,
    pub avg_output_tokens: usize,
    pub total_cost_usd: f64,
    pub avg_cost_usd: f64,
    pub total_pass1_candidates: usize,
    pub total_confirmed_findings: usize,
    pub total_dismissed_findings: usize,
    pub overall_suppression_pct: u8,
    pub avg_exact_anchor_rate_pct: u8,
    pub avg_coverage_rate_pct: u8,
    pub gate_failures: usize,
}

/// Convert anchored review findings into a SARIF 2.1.0 JSON document suitable for
/// GitHub Code Scanning (`github/codeql-action/upload-sarif`) and GitLab SAST.
// trace:STORY-3120 | ai:antigravity
pub fn findings_to_sarif_json(findings: &[RawReviewFinding]) -> serde_json::Value {
    findings_to_sarif_json_with_telemetry(findings, &[], None)
}

/// Convert confirmed and Pass-2 dismissed review findings (across all categories:
/// behavioral/spec-acceptance, logic, concurrency, security, error-handling, and
/// language/hardware playbooks) plus optional review telemetry into a self-contained
/// SARIF 2.1.0 JSON document.
///
/// Dismissed Pass-1 candidates are recorded with standard SARIF 2.1.0 §3.35
/// `suppressions` (`kind: "external"`, `status: "accepted"`, `justification`) so
/// Code Scanning gates ignore them while the SARIF database retains full false-positive
/// provenance.
// trace:STORY-3120 | ai:antigravity
pub fn findings_to_sarif_json_with_telemetry(
    findings: &[RawReviewFinding],
    dismissed: &[DismissedReviewFinding],
    telemetry: Option<&ReviewTelemetryRecord>,
) -> serde_json::Value {
    let mut rules_map: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    let mut results: Vec<serde_json::Value> = Vec::with_capacity(findings.len() + dismissed.len());

    let mut append_finding = |f: &RawReviewFinding, dismissal_reason: Option<&str>| {
        let cat_clean = if f.category.trim().is_empty() {
            "general"
        } else {
            f.category.trim()
        };
        let rule_id = format!("aida.review.{cat_clean}");
        rules_map.entry(rule_id.clone()).or_insert_with(|| {
            serde_json::json!({
                "id": rule_id,
                "name": format!("AidaReview{}", cat_clean),
                "shortDescription": {
                    "text": format!("AIDA deterministic review finding ({cat_clean})")
                }
            })
        });

        let level = match f.severity.trim().to_ascii_lowercase().as_str() {
            "critical" | "high" | "error" => "error",
            "medium" | "warning" | "warn" => "warning",
            _ => "note",
        };

        let start_line = f.start_line.unwrap_or(1).max(1);
        let end_line = f.end_line.unwrap_or(start_line).max(start_line);

        let mut message_text = f.message.clone();
        if let Some(suggestion) = f.suggestion_code.as_deref() {
            if !suggestion.trim().is_empty() {
                message_text.push_str("\n\nSuggested fix:\n");
                message_text.push_str(suggestion.trim());
            }
        }

        let anchor_status = f.anchor_status.as_deref().unwrap_or("unanchored");
        let mut result_obj = serde_json::json!({
            "ruleId": rule_id,
            "level": level,
            "message": {
                "text": message_text
            },
            "locations": [
                {
                    "physicalLocation": {
                        "artifactLocation": {
                            "uri": f.path.replace('\\', "/"),
                            "uriBaseId": "%SRCROOT%"
                        },
                        "region": {
                            "startLine": start_line,
                            "endLine": end_line
                        }
                    }
                }
            ],
            "properties": {
                "category": cat_clean,
                "severity": f.severity.trim().to_ascii_lowercase(),
                "anchorStatus": anchor_status,
                "disposition": if dismissal_reason.is_some() { "dismissed_by_challenger" } else { "confirmed" }
            }
        });

        if let Some(reason) = dismissal_reason {
            result_obj["suppressions"] = serde_json::json!([
                {
                    "kind": "external",
                    "status": "accepted",
                    "justification": reason
                }
            ]);
            result_obj["properties"]["challengerReason"] =
                serde_json::Value::String(reason.to_string());
        }

        results.push(result_obj);
    };

    for f in findings {
        append_finding(f, None);
    }
    for d in dismissed {
        append_finding(&d.finding, Some(&d.dismissal_reason));
    }

    let rules: Vec<serde_json::Value> = rules_map.into_values().collect();
    let mut run_obj = serde_json::json!({
        "tool": {
            "driver": {
                "name": "aida-review-engine",
                "informationUri": "https://github.com/joemooney/aida",
                "rules": rules
            }
        },
        "results": results
    });

    if let Some(t) = telemetry {
        run_obj["invocations"] = serde_json::json!([
            {
                "executionSuccessful": !t.gate_failed,
                "properties": {
                    "reviewId": t.review_id,
                    "abArm": t.ab_arm,
                    "prepareElapsedMs": t.prepare_elapsed_ms,
                    "finderElapsedMs": t.finder_elapsed_ms,
                    "challengerElapsedMs": t.challenger_elapsed_ms,
                    "verifyElapsedMs": t.verify_elapsed_ms,
                    "totalElapsedMs": t.total_elapsed_ms,
                    "estimatedInputTokens": t.estimated_input_tokens,
                    "estimatedOutputTokens": t.estimated_output_tokens,
                    "estimatedCostUsd": t.estimated_cost_usd
                }
            }
        ]);
        run_obj["properties"] = serde_json::json!({
            "aidaTelemetry": t
        });
    }

    serde_json::json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [run_obj]
    })
}

/// Summary of the two-pass Red-Team Finder -> Blue-Team Skeptical Challenger pipeline.
// trace:STORY-3120 | ai:antigravity
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdversarialReviewSummary {
    pub finder_agent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finder_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finder_effort: Option<String>,
    pub challenger_agent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub challenger_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub challenger_effort: Option<String>,
    pub candidate_findings: usize,
    pub confirmed_findings: usize,
    pub dismissed_findings: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detected_author_agents: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dismissed_details: Vec<DismissedReviewFinding>,
}

/// Format a consolidated GitHub PR / GitLab MR Markdown review comment from the
/// classified inventory, optional coverage report, linked spec `(spec_id, title)` list,
/// and anchored findings.
// trace:STORY-3120 | ai:antigravity
pub fn format_forge_review_markdown(
    mode: &str,
    head_ref: Option<&str>,
    inventory: &ClassifiedInventory,
    coverage: Option<&CoverageVerificationReport>,
    linked_specs: &[(String, String)],
    findings: &[RawReviewFinding],
) -> String {
    format_forge_review_markdown_with_adversarial(
        mode,
        head_ref,
        inventory,
        coverage,
        linked_specs,
        findings,
        None,
    )
}

/// Format a GitHub PR / GitLab MR Markdown review comment with optional adversarial
/// Finder -> Challenger verification summary.
// trace:STORY-3120 | ai:antigravity
pub fn format_forge_review_markdown_with_adversarial(
    mode: &str,
    head_ref: Option<&str>,
    inventory: &ClassifiedInventory,
    coverage: Option<&CoverageVerificationReport>,
    linked_specs: &[(String, String)],
    findings: &[RawReviewFinding],
    adversarial: Option<&AdversarialReviewSummary>,
) -> String {
    let mut out = String::new();
    out.push_str("## AIDA Deterministic Code Review\n\n");

    let head_short = head_ref
        .map(|h| if h.len() > 12 { &h[..12] } else { h })
        .unwrap_or("workspace");
    let inv_short = if inventory.inventory_sha256.len() > 12 {
        &inventory.inventory_sha256[..12]
    } else {
        &inventory.inventory_sha256
    };

    out.push_str(&format!(
        "- **Mode**: `{mode}` | **Head**: `{head_short}` | **Inventory SHA-256**: `{inv_short}`\n"
    ));
    out.push_str(&format!(
        "- **Reviewable files**: {} | **Excluded files**: {}\n",
        inventory.reviewable_files.len(),
        inventory.excluded_files.len()
    ));

    if let Some(cov) = coverage {
        let status_label = if cov.passed { "PASSED" } else { "INCOMPLETE" };
        out.push_str(&format!(
            "- **File coverage**: **{status_label}** ({}% — {} reviewed, {} skipped of {})\n",
            cov.coverage_rate_pct, cov.reviewed_count, cov.skipped_count, cov.total_reviewable
        ));
    }

    if let Some(adv) = adversarial {
        let finder_label = match &adv.finder_model {
            Some(m) => format!("`{}` (`{m}`)", adv.finder_agent),
            None => format!("`{}`", adv.finder_agent),
        };
        let challenger_label = match &adv.challenger_model {
            Some(m) => format!("`{}` (`{m}`)", adv.challenger_agent),
            None => format!("`{}`", adv.challenger_agent),
        };
        out.push_str(&format!(
            "- **Adversarial verification**: Finder {finder_label} → Challenger {challenger_label} — **{}** candidate(s) challenged → **{}** confirmed, **{}** dismissed as false positive(s)\n",
            adv.candidate_findings, adv.confirmed_findings, adv.dismissed_findings
        ));
        if !adv.detected_author_agents.is_empty() {
            let authors: Vec<String> = adv
                .detected_author_agents
                .iter()
                .map(|a| format!("`{a}`"))
                .collect();
            out.push_str(&format!(
                "- **Authorship separation**: Detected commit/trace author(s) {} → Reviewed by `{}`\n",
                authors.join(", "),
                adv.finder_agent
            ));
        }
    }

    if !linked_specs.is_empty() {
        let specs_str: Vec<String> = linked_specs
            .iter()
            .map(|(id, title)| format!("`{id}` ({title})"))
            .collect();
        out.push_str(&format!(
            "- **Linked AIDA specs**: {}\n",
            specs_str.join(", ")
        ));
    }
    out.push('\n');

    if findings.is_empty() {
        out.push_str("### Findings\n\nNo actionable defects reported.\n");
        return out;
    }

    let mut crit_high = 0usize;
    let mut medium = 0usize;
    let mut low = 0usize;
    for f in findings {
        match f.severity.trim().to_ascii_lowercase().as_str() {
            "critical" | "high" => crit_high += 1,
            "medium" => medium += 1,
            _ => low += 1,
        }
    }

    out.push_str(&format!(
        "### Findings ({} critical/high, {} medium, {} low)\n\n",
        crit_high, medium, low
    ));

    for f in findings {
        let loc = match (f.start_line, f.end_line) {
            (Some(s), Some(e)) if s != e => format!("{}:{s}-{e}", f.path),
            (Some(s), _) => format!("{}:{s}", f.path),
            _ => f.path.clone(),
        };
        let sev = f.severity.trim().to_ascii_uppercase();
        let cat = f.category.trim();
        out.push_str(&format!(
            "- **[{}] `{}`** (`{}`) — {}\n",
            sev,
            loc,
            cat,
            f.message.trim()
        ));
        if let Some(sugg) = f.suggestion_code.as_deref() {
            if !sugg.trim().is_empty() {
                out.push_str("  ```suggestion\n");
                for line in sugg.trim_end().lines() {
                    out.push_str("  ");
                    out.push_str(line);
                    out.push('\n');
                }
                out.push_str("  ```\n");
            }
        }
    }

    out
}

/// Envelope parsed from an automated batch review runner's stdout.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BatchRunnerOutput {
    #[serde(default)]
    pub findings: Vec<RawReviewFinding>,
    #[serde(default)]
    pub files: Vec<FileCoverageEntry>,
}

/// Parse JSON output from an automated batch review runner (`claude --print` or
/// `AIDA_REVIEW_RUNNER_CMD`). Accepts either a `{ "findings": [...], "files": [...] }`
/// object or a bare `[ RawReviewFinding, ... ]` array, stripping surrounding markdown
/// code fences if present.
// trace:STORY-3120 | ai:antigravity
pub fn parse_runner_findings_json(
    raw: &str,
) -> Result<(Vec<RawReviewFinding>, CoverageLedger), String> {
    let trimmed = extract_json_payload(raw);
    if trimmed.starts_with('[') {
        let findings: Vec<RawReviewFinding> = serde_json::from_str(trimmed)
            .map_err(|e| format!("failed to parse runner findings array: {e}"))?;
        return Ok((findings, CoverageLedger::default()));
    }
    let env: BatchRunnerOutput = serde_json::from_str(trimmed)
        .map_err(|e| format!("failed to parse runner output object: {e}"))?;
    Ok((env.findings, CoverageLedger { files: env.files }))
}

/// Single verdict emitted by the Pass 2 Blue-Team Skeptical Challenger.
// trace:STORY-3120 | ai:antigravity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChallengerVerdictRow {
    pub index: usize,
    /// `"confirmed"` or `"dismissed"` (also accepts `"rejected"` / `"false_positive"`).
    pub verdict: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub refined_message: Option<String>,
}

/// JSON envelope returned by the Pass 2 Blue-Team Skeptical Challenger.
// trace:STORY-3120 | ai:antigravity
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChallengerOutput {
    #[serde(default)]
    pub verdicts: Vec<ChallengerVerdictRow>,
}

/// Apply Pass 2 Blue-Team Challenger verdicts to Pass 1 candidate findings.
/// Returns `(confirmed_findings, dismissed_count)`.
// trace:STORY-3120 | ai:antigravity
pub fn apply_adversarial_challenger_json(
    candidates: Vec<RawReviewFinding>,
    raw_challenger_output: &str,
) -> Result<(Vec<RawReviewFinding>, usize), String> {
    let (confirmed, dismissed) =
        apply_adversarial_challenger_with_dismissed(candidates, raw_challenger_output)?;
    Ok((confirmed, dismissed.len()))
}

/// Apply Pass 2 Blue-Team Challenger verdicts to Pass 1 candidate findings,
/// returning `(confirmed_findings, dismissed_findings_with_reasons)` so the SARIF 2.1.0
/// archive and telemetry database can record both confirmed findings and Pass-2
/// suppressed false positives.
// trace:STORY-3120 | ai:antigravity
pub fn apply_adversarial_challenger_with_dismissed(
    candidates: Vec<RawReviewFinding>,
    raw_challenger_output: &str,
) -> Result<(Vec<RawReviewFinding>, Vec<DismissedReviewFinding>), String> {
    if candidates.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let trimmed = extract_json_payload(raw_challenger_output);
    let parsed: ChallengerOutput = serde_json::from_str(trimmed)
        .map_err(|e| format!("failed to parse challenger verdicts JSON: {e}"))?;

    let mut verdict_by_idx: BTreeMap<usize, &ChallengerVerdictRow> = BTreeMap::new();
    for v in &parsed.verdicts {
        verdict_by_idx.insert(v.index, v);
    }

    let mut confirmed = Vec::with_capacity(candidates.len());
    let mut dismissed = Vec::new();

    for (idx, mut finding) in candidates.into_iter().enumerate() {
        if let Some(v) = verdict_by_idx.get(&idx) {
            let norm = v.verdict.trim().to_ascii_lowercase();
            if matches!(
                norm.as_str(),
                "dismissed"
                    | "dismiss"
                    | "rejected"
                    | "reject"
                    | "false_positive"
                    | "false-positive"
            ) {
                let reason = v
                    .reason
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("Dismissed as false positive by Pass-2 Blue-Team Challenger")
                    .to_string();
                dismissed.push(DismissedReviewFinding {
                    finding,
                    dismissal_reason: reason,
                });
                continue;
            }
            if let Some(refined) = v.refined_message.as_deref() {
                if !refined.trim().is_empty() {
                    finding.message = refined.trim().to_string();
                }
            }
        }
        confirmed.push(finding);
    }

    Ok((confirmed, dismissed))
}

/// Estimate USD cost for a review invocation based on agent, model family,
/// and input/output token counts.
// trace:STORY-3120 | ai:antigravity
pub fn estimate_review_cost_usd(
    agent: Option<&str>,
    model: Option<&str>,
    input_tokens: usize,
    output_tokens: usize,
) -> f64 {
    let model_lower = model.unwrap_or("").to_ascii_lowercase();
    let agent_lower = agent.unwrap_or("").to_ascii_lowercase();
    // Standard approximate per-1M-token rates (input_per_1m, output_per_1m) in USD
    let (in_rate, out_rate) = if model_lower.contains("opus") {
        (15.0, 75.0)
    } else if model_lower.contains("haiku")
        || model_lower.contains("mini")
        || model_lower.contains("flash")
    {
        (0.80, 4.0)
    } else if model_lower.contains("o3") {
        (10.0, 40.0)
    } else if model_lower.contains("codex") || agent_lower == "codex" {
        (2.50, 10.0)
    } else if agent_lower == "gemini" || agent_lower == "agy" {
        (1.25, 5.0)
    } else {
        // Default Claude Sonnet / frontier baseline
        (3.0, 15.0)
    };
    let raw = (input_tokens as f64 / 1_000_000.0) * in_rate
        + (output_tokens as f64 / 1_000_000.0) * out_rate;
    (raw * 100_000.0).round() / 100_000.0
}

/// Resolve the A/B experiment arm label and whether `--adversarial` Pass 2 should run.
///
/// When `requested_arm` is `Some("auto")` and `run_review` is `true` (without an explicit
/// `--adversarial` flag), deterministically assigns 50/50 between `("ab-single-pass", false)`
/// and `("ab-adversarial", true)` using `pr_number` parity or the first hex byte of
/// `head_ref` / `inventory_sha256`.
// trace:STORY-3120 | ai:antigravity
pub fn resolve_ab_arm_assignment(
    requested_arm: Option<&str>,
    pr_number: Option<u64>,
    head_ref: Option<&str>,
    inventory_sha256: &str,
    run_review: bool,
    explicit_adversarial: bool,
) -> (String, bool) {
    let cleaned = requested_arm.map(str::trim).filter(|s| !s.is_empty());
    if !run_review {
        return (
            cleaned.unwrap_or("prepare-only").to_string(),
            explicit_adversarial,
        );
    }
    if let Some(arm) = cleaned {
        if arm.eq_ignore_ascii_case("auto") {
            if explicit_adversarial {
                return ("ab-adversarial".to_string(), true);
            }
            let bucket_is_odd = if let Some(pr) = pr_number {
                pr % 2 == 1
            } else {
                let seed = head_ref
                    .filter(|h| !h.is_empty())
                    .unwrap_or(inventory_sha256);
                seed.bytes().next().unwrap_or(0) % 2 == 1
            };
            return if bucket_is_odd {
                ("ab-adversarial".to_string(), true)
            } else {
                ("ab-single-pass".to_string(), false)
            };
        }
        let adv = explicit_adversarial || arm.to_ascii_lowercase().contains("adversarial");
        return (arm.to_string(), adv);
    }
    if explicit_adversarial {
        ("adversarial".to_string(), true)
    } else {
        ("single-pass".to_string(), false)
    }
}

/// Aggregate a slice of `ReviewTelemetryRecord` entries by `ab_arm` for A/B comparison.
// trace:STORY-3120 | ai:antigravity
pub fn summarize_review_telemetry_by_arm(
    records: &[ReviewTelemetryRecord],
) -> Vec<ReviewArmSummary> {
    let mut grouped: BTreeMap<String, Vec<&ReviewTelemetryRecord>> = BTreeMap::new();
    for r in records {
        grouped.entry(r.ab_arm.clone()).or_default().push(r);
    }

    let mut summaries = Vec::with_capacity(grouped.len());
    for (arm, rows) in grouped {
        let n = rows.len();
        if n == 0 {
            continue;
        }
        let sum_total_ms: u64 = rows.iter().map(|r| r.total_elapsed_ms).sum();
        let sum_prepare_ms: u64 = rows.iter().map(|r| r.prepare_elapsed_ms).sum();
        let sum_finder_ms: u64 = rows.iter().map(|r| r.finder_elapsed_ms).sum();
        let sum_challenger_ms: u64 = rows.iter().map(|r| r.challenger_elapsed_ms).sum();
        let sum_in_tok: usize = rows.iter().map(|r| r.estimated_input_tokens).sum();
        let sum_out_tok: usize = rows.iter().map(|r| r.estimated_output_tokens).sum();
        let total_cost: f64 = rows.iter().map(|r| r.estimated_cost_usd).sum();
        let total_candidates: usize = rows.iter().map(|r| r.pass1_candidate_findings).sum();
        let total_confirmed: usize = rows.iter().map(|r| r.confirmed_findings).sum();
        let total_dismissed: usize = rows.iter().map(|r| r.dismissed_findings).sum();
        let overall_suppression_pct = if total_candidates > 0 {
            ((total_dismissed * 100) / total_candidates).min(100) as u8
        } else {
            0
        };
        let sum_anchor_pct: usize = rows.iter().map(|r| r.exact_anchor_rate_pct as usize).sum();
        let mut cov_sum = 0usize;
        let mut cov_cnt = 0usize;
        for r in &rows {
            if let Some(c) = r.coverage_rate_pct {
                cov_sum += c as usize;
                cov_cnt += 1;
            }
        }
        let avg_cov = if cov_cnt > 0 {
            (cov_sum / cov_cnt) as u8
        } else {
            100
        };
        let gate_failures = rows.iter().filter(|r| r.gate_failed).count();

        summaries.push(ReviewArmSummary {
            ab_arm: arm,
            runs: n,
            avg_total_ms: sum_total_ms / (n as u64),
            avg_prepare_ms: sum_prepare_ms / (n as u64),
            avg_finder_ms: sum_finder_ms / (n as u64),
            avg_challenger_ms: sum_challenger_ms / (n as u64),
            avg_input_tokens: sum_in_tok / n,
            avg_output_tokens: sum_out_tok / n,
            total_cost_usd: (total_cost * 100_000.0).round() / 100_000.0,
            avg_cost_usd: ((total_cost / (n as f64)) * 100_000.0).round() / 100_000.0,
            total_pass1_candidates: total_candidates,
            total_confirmed_findings: total_confirmed,
            total_dismissed_findings: total_dismissed,
            overall_suppression_pct,
            avg_exact_anchor_rate_pct: (sum_anchor_pct / n) as u8,
            avg_coverage_rate_pct: avg_cov,
            gate_failures,
        });
    }
    summaries
}

/// Format an A/B telemetry comparison report in Markdown from recorded review telemetry rows.
// trace:STORY-3120 | ai:antigravity
pub fn format_review_telemetry_report_markdown(records: &[ReviewTelemetryRecord]) -> String {
    let mut out = String::new();
    out.push_str("# AIDA Code Review Telemetry & A/B Comparison Report\n\n");
    out.push_str(&format!(
        "- **Total recorded review runs**: {}\n\n",
        records.len()
    ));
    if records.is_empty() {
        out.push_str(
            "No review telemetry records found in `.aida/review-telemetry/reviews.jsonl`.\n",
        );
        return out;
    }

    let summaries = summarize_review_telemetry_by_arm(records);
    out.push_str("## A/B Arm Summary\n\n");
    out.push_str("| A/B Arm | Runs | Avg Total (ms) | Avg Finder / Challenger (ms) | Avg Tokens (in/out) | Avg Cost ($) | Candidates → Confirmed (Dismissed %) | Exact Anchor % | Avg Coverage % |\n");
    out.push_str("| :--- | :---: | :---: | :---: | :---: | :---: | :---: | :---: | :---: |\n");
    for s in &summaries {
        out.push_str(&format!(
            "| `{}` | {} | {} ms | {} / {} ms | {} / {} | ${:.4} | {} → {} ({}% suppressed) | {}% | {}% |\n",
            s.ab_arm,
            s.runs,
            s.avg_total_ms,
            s.avg_finder_ms,
            s.avg_challenger_ms,
            s.avg_input_tokens,
            s.avg_output_tokens,
            s.avg_cost_usd,
            s.total_pass1_candidates,
            s.total_confirmed_findings,
            s.overall_suppression_pct,
            s.avg_exact_anchor_rate_pct,
            s.avg_coverage_rate_pct
        ));
    }
    out.push('\n');

    let mut cat_totals: BTreeMap<String, usize> = BTreeMap::new();
    for r in records {
        for (cat, cnt) in &r.counts_by_category {
            *cat_totals.entry(cat.clone()).or_insert(0) += cnt;
        }
    }
    if !cat_totals.is_empty() {
        out.push_str("## Confirmed Findings by Category (All Stored in SARIF 2.1.0)\n\n");
        out.push_str("| Category (`aida.review.<category>`) | Confirmed Findings |\n");
        out.push_str("| :--- | :---: |\n");
        for (cat, cnt) in cat_totals {
            out.push_str(&format!("| `aida.review.{cat}` | {cnt} |\n"));
        }
        out.push('\n');
    }

    out
}

/// Numeric rank for a review finding severity (`critical`=4, `high`=3, `medium`=2, `low`=1).
// trace:STORY-3120 | ai:antigravity
pub fn severity_rank(severity: &str) -> Option<u8> {
    match severity.trim().to_ascii_lowercase().as_str() {
        "critical" => Some(4),
        "high" => Some(3),
        "medium" => Some(2),
        "low" | "info" | "note" => Some(1),
        _ => None,
    }
}

/// Return all findings whose severity meets or exceeds `threshold` (`critical`, `high`, `medium`, `low`).
// trace:STORY-3120 | ai:antigravity
pub fn findings_exceeding_threshold<'a>(
    findings: &'a [RawReviewFinding],
    threshold: &str,
) -> Result<Vec<&'a RawReviewFinding>, String> {
    let min_rank = severity_rank(threshold).ok_or_else(|| {
        format!(
            "invalid --fail-on severity `{threshold}` (expected `critical`, `high`, `medium`, or `low`)"
        )
    })?;
    Ok(findings
        .iter()
        .filter(|f| severity_rank(&f.severity).unwrap_or(1) >= min_rank)
        .collect())
}

/// Detect AI author agent tokens (`claude`, `codex`, `agy`, `gemini`) from commit messages
/// (`[AI:tool]` or `[AI:tool1+tool2:med]`) and file trace comments (`// trace:SPEC | ai:tool`).
// trace:STORY-3120 | ai:antigravity
pub fn detect_ai_authors(commit_messages: &[String], file_contents: &[&str]) -> Vec<String> {
    static COMMIT_AI_RE: once_cell::sync::Lazy<Regex> = once_cell::sync::Lazy::new(|| {
        Regex::new(r"(?i)\[AI:([A-Za-z0-9_+-]+)(?::[A-Za-z]+)?\]")
            .expect("static commit AI regex is valid")
    });
    static TRACE_AI_RE: once_cell::sync::Lazy<Regex> = once_cell::sync::Lazy::new(|| {
        Regex::new(r"(?i)trace:[A-Za-z0-9_-]+\s*\|\s*ai:([A-Za-z0-9_+-]+)")
            .expect("static trace AI regex is valid")
    });

    let mut authors = BTreeSet::new();
    let mut record_token = |raw_group: &str| {
        for part in raw_group.split('+') {
            match part.trim().to_ascii_lowercase().as_str() {
                "claude" => {
                    authors.insert("claude".to_string());
                }
                "codex" => {
                    authors.insert("codex".to_string());
                }
                "antigravity" | "agy" => {
                    authors.insert("agy".to_string());
                }
                "gemini" => {
                    authors.insert("gemini".to_string());
                }
                _ => {}
            }
        }
    };

    for msg in commit_messages {
        for cap in COMMIT_AI_RE.captures_iter(msg) {
            if let Some(m) = cap.get(1) {
                record_token(m.as_str());
            }
        }
    }
    for body in file_contents {
        for cap in TRACE_AI_RE.captures_iter(body) {
            if let Some(m) = cap.get(1) {
                record_token(m.as_str());
            }
        }
    }

    authors.into_iter().collect()
}

fn extract_json_payload(raw: &str) -> &str {
    let s = raw.trim();
    if let Some(start_fence) = s.find("```json") {
        let after = &s[start_fence + 7..];
        if let Some(end_fence) = after.find("```") {
            return after[..end_fence].trim();
        }
    }
    if let Some(start_fence) = s.find("```") {
        let after = &s[start_fence + 3..];
        if let Some(end_fence) = after.find("```") {
            return after[..end_fence].trim();
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    // trace:TASK-3867 | ai:codex
    fn diff_entries(paths: &[&str], binary: bool) -> Vec<RawDiffEntry> {
        paths
            .iter()
            .map(|p| RawDiffEntry {
                path: (*p).into(),
                old_path: None,
                kind: FileChangeKind::Modified,
                additions: 0,
                deletions: 0,
                patch_bytes: 0,
                is_binary: binary,
            })
            .collect()
    }

    fn inventory(paths: &[&str], binary: bool) -> ClassifiedInventory {
        let entries = diff_entries(paths, binary);
        classify_diff_inventory(&entries, &Default::default(), &[], usize::MAX, |_| false)
    }

    #[test]
    fn h2a_digest_frames_paths() {
        let pair = inventory(&["a", "b"], true).inventory_sha256;
        assert_eq!(pair, inventory(&["b", "a"], true).inventory_sha256);
        assert_ne!(
            pair,
            inventory(&["a:binary_file\nexcluded:b"], true).inventory_sha256
        );
    }

    #[test]
    fn h2a_duplicates_are_ambiguous() {
        let row = FileCoverageEntry {
            path: "a".into(),
            status: FileCoverageStatus::Reviewed,
            reason: None,
        };
        let coverage = verify_coverage_ledger(
            &inventory(&["a"], false),
            &CoverageLedger {
                files: vec![row.clone(), row],
            },
        );
        assert!(!coverage.passed);
        assert_eq!(coverage.reviewed_count, 0);
        assert_eq!(coverage.ambiguous_files, ["a"]);
        let obs = ReviewObservation {
            id: "obs-1".into(),
            path: "a".into(),
            start_line: 1,
            end_line: 1,
            content_sha256: String::new(),
            has_error: false,
        };
        let citations = verify_observation_citations("[[obs-1]]", &[obs.clone(), obs]);
        assert!(!citations.valid);
        assert_eq!(citations.ambiguous_ids, ["obs-1"]);
    }

    #[test]
    fn h2a_token_overflow() {
        assert_eq!(estimate_file_tokens(usize::MAX, 0, 0), usize::MAX / 4 + 1);
        assert_eq!(estimate_file_tokens(0, usize::MAX, 1), usize::MAX);
        let mut files = inventory(&["a.rs", "a_test.rs"], false).reviewable_files;
        files[0].estimated_tokens = usize::MAX;
        let batches = bundle_review_batches(&files, usize::MAX, 2).unwrap();
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].estimated_tokens, usize::MAX);
    }

    #[test]
    fn h2a_budget_is_a_cap() {
        let files = inventory(&["a", "b"], false).reviewable_files;
        assert_eq!(bundle_review_batches(&files, 32, 2).unwrap().len(), 2);
        let mut entries = diff_entries(&["large.rs", "small.rs"], false);
        entries[0].additions = 3;
        let inv = classify_diff_inventory(&entries, &Default::default(), &[], 32, |_| false);
        assert_eq!(inv.excluded_files.len(), 1);
        assert_eq!(inv.excluded_files[0].path, "large.rs");
        assert_eq!(
            inv.excluded_files[0].reason,
            "oversize: 36 estimated tokens > cap 32"
        );
        assert_eq!(
            verify_coverage_ledger(&inv, &CoverageLedger::default()).not_reviewable_by_batch,
            inv.excluded_files
        );
        let batches = bundle_review_batches(&inv.reviewable_files, 32, 2).unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].files, ["small.rs"]);
    }

    #[test]
    fn h2a_single_bracket_citations() {
        let citations = verify_observation_citations("[obs-1] [[obs-2]] [obs-1] obs-3", &[]);
        assert!(!citations.valid);
        assert_eq!(citations.unknown_ids, ["obs-1", "obs-2"]);
    }

    // trace:TASK-3866 | ai:codex
    #[test]
    fn credential_components_at_any_depth() {
        for dir in [".aws", ".ssh"] {
            for prefix in ["", "one/", "one/two/"] {
                assert_eq!(
                    classify_path_exclusion(&format!("{prefix}{dir}/config"), &[]),
                    Some(ExclusionReason::SecretSensitivePath)
                );
            }
            assert_eq!(
                classify_path_exclusion(&format!("{dir}-docs/config"), &[]),
                None
            );
        }
    }

    #[test]
    fn rename_excludes_either_path() {
        for (old, new) in [(".ssh/config", "public"), ("public", ".ssh/config")] {
            let entry = RawDiffEntry {
                path: new.into(),
                old_path: Some(old.into()),
                kind: FileChangeKind::Renamed,
                additions: 0,
                deletions: 0,
                is_binary: false,
                patch_bytes: 0,
            };
            assert_eq!(
                classify_exclusion(&entry, &[]),
                Some(ExclusionReason::SecretSensitivePath)
            );
        }
    }

    #[test]
    fn runtime_prefix_requires_exact_components() {
        for path in [".aida/cache", ".aida/cache/entry", ".aida/cache-db/entry"] {
            assert_eq!(
                classify_path_exclusion(path, &[]),
                Some(ExclusionReason::AidaSubstrate)
            );
        }
        assert_eq!(
            classify_path_exclusion(".aida/cache-policy.toml", &[]),
            None
        );
    }

    #[test]
    fn config_defaults_only_when_absent() {
        let root = tempfile::tempdir().unwrap();
        let load = || ProjectReviewRulesConfig::load_from_root(root.path());
        assert!(load().unwrap().exclude.is_empty());
        let dir = root.path().join(".aida");
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("review-rules.toml");
        std::fs::write(&path, "exclude = ['docs/**'] # comment").unwrap();
        assert_eq!(load().unwrap().exclude, ["docs/**"]);
        for bytes in [b"exclude = [".as_slice(), &[0xff]] {
            std::fs::write(&path, bytes).unwrap();
            assert!(load()
                .unwrap_err()
                .to_string()
                .contains(path.to_str().unwrap()));
        }
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink("missing", &path).unwrap();
        assert!(load()
            .unwrap_err()
            .to_string()
            .contains(path.to_str().unwrap()));
    }

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
        let inv =
            classify_diff_inventory(&entries, &registry, &user_excludes, usize::MAX, |path| {
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
        let batches = bundle_review_batches(&files, 2000, 4).unwrap();
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

    // trace:STORY-3120 | ai:antigravity
    #[test]
    fn test_findings_to_sarif_json_and_forge_markdown_and_runner_parse() {
        let raw_runner = r#"```json
{
  "findings": [
    {
      "path": "src/auth.rs",
      "severity": "high",
      "category": "fail-open",
      "message": "Swallowed verification error allows unauthenticated access.",
      "existing_code": "let _ = verify_token(tok);",
      "suggestion_code": "verify_token(tok)?;",
      "start_line": 42,
      "end_line": 42
    }
  ],
  "files": [
    { "path": "src/auth.rs", "status": "reviewed" }
  ]
}
```"#;

        let (findings, ledger) =
            parse_runner_findings_json(raw_runner).expect("runner JSON should parse");
        assert_eq!(findings.len(), 1);
        assert_eq!(ledger.files.len(), 1);
        assert_eq!(findings[0].path, "src/auth.rs");

        let sarif = findings_to_sarif_json(&findings);
        assert_eq!(sarif["version"], "2.1.0");
        let result0 = &sarif["runs"][0]["results"][0];
        assert_eq!(result0["ruleId"], "aida.review.fail-open");
        assert_eq!(result0["level"], "error");
        assert_eq!(
            result0["locations"][0]["physicalLocation"]["artifactLocation"]["uri"],
            "src/auth.rs"
        );
        assert_eq!(
            result0["locations"][0]["physicalLocation"]["region"]["startLine"],
            42
        );

        let inv = inventory(&["src/auth.rs"], false);
        let cov = verify_coverage_ledger(&inv, &ledger);
        let adv_summary = AdversarialReviewSummary {
            finder_agent: "codex".to_string(),
            finder_model: Some("gpt-5.3-codex".to_string()),
            finder_effort: Some("medium".to_string()),
            challenger_agent: "codex".to_string(),
            challenger_model: Some("o3".to_string()),
            challenger_effort: Some("high".to_string()),
            candidate_findings: 2,
            confirmed_findings: 1,
            dismissed_findings: 1,
            detected_author_agents: vec!["claude".to_string()],
            dismissed_details: Vec::new(),
        };
        let md = format_forge_review_markdown_with_adversarial(
            "range",
            Some("deadbeef12345678"),
            &inv,
            Some(&cov),
            &[("STORY-3120".to_string(), "Turnkey CI review".to_string())],
            &findings,
            Some(&adv_summary),
        );
        assert!(md.contains("## AIDA Deterministic Code Review"));
        assert!(md.contains("**PASSED** (100%"));
        assert!(md.contains("Adversarial verification"));
        assert!(md.contains("Authorship separation"));
        assert!(md.contains("`codex` (`o3`)"));
        assert!(md.contains("`src/auth.rs:42`"));
        assert!(md.contains("verify_token(tok)?;"));

        let exceeded = findings_exceeding_threshold(&findings, "high").unwrap();
        assert_eq!(exceeded.len(), 1);
        assert!(findings_exceeding_threshold(&findings, "critical")
            .unwrap()
            .is_empty());

        let detected = detect_ai_authors(
            &["[AI:claude+antigravity:med] feat(auth): add token verify (STORY-3120)".to_string()],
            &["// trace:STORY-3120 | ai:codex\npub fn verify() {}"],
        );
        assert_eq!(
            detected,
            vec!["agy".to_string(), "claude".to_string(), "codex".to_string()]
        );

        let candidates = vec![
            findings[0].clone(),
            RawReviewFinding {
                path: "src/auth.rs".to_string(),
                severity: "low".to_string(),
                category: "bug".to_string(),
                message: "Hypothetical nitpick already handled by caller.".to_string(),
                existing_code: None,
                suggestion_code: None,
                start_line: None,
                end_line: None,
                anchor_status: None,
            },
        ];
        let challenger_json = r#"```json
{
  "verdicts": [
    { "index": 0, "verdict": "confirmed", "reason": "Real fail-open defect." },
    { "index": 1, "verdict": "dismissed", "reason": "Caller already validates." }
  ]
}
```"#;
        let (confirmed, dismissed_details) =
            apply_adversarial_challenger_with_dismissed(candidates, challenger_json).unwrap();
        assert_eq!(confirmed.len(), 1);
        assert_eq!(dismissed_details.len(), 1);
        assert_eq!(
            dismissed_details[0].dismissal_reason,
            "Caller already validates."
        );

        let mut cat_map = BTreeMap::new();
        cat_map.insert("fail-open".to_string(), 1);
        let sample_telemetry = ReviewTelemetryRecord {
            review_id: "rev-001".to_string(),
            timestamp: "2026-11-25T17:30:00Z".to_string(),
            ab_arm: "ab-adversarial".to_string(),
            mode: "range".to_string(),
            pr_number: Some(4575),
            base_ref: Some("main".to_string()),
            head_ref: Some("deadbeef".to_string()),
            inventory_sha256: "inv123".to_string(),
            linked_specs: vec!["STORY-3120".to_string()],
            detected_author_agents: vec!["claude".to_string()],
            finder_agent: Some("codex".to_string()),
            finder_model: Some("gpt-5.3-codex".to_string()),
            finder_effort: Some("medium".to_string()),
            challenger_agent: Some("codex".to_string()),
            challenger_model: Some("o3".to_string()),
            challenger_effort: Some("high".to_string()),
            reviewable_files: 1,
            excluded_files: 0,
            batch_count: 1,
            total_added_lines: 25,
            total_removed_lines: 4,
            active_playbooks: vec!["rust".to_string()],
            prepare_elapsed_ms: 15,
            finder_elapsed_ms: 1200,
            challenger_elapsed_ms: 950,
            verify_elapsed_ms: 3,
            total_elapsed_ms: 2168,
            finder_prompt_bytes: 4000,
            finder_response_bytes: 600,
            challenger_prompt_bytes: 3200,
            challenger_response_bytes: 400,
            estimated_input_tokens: 1800,
            estimated_output_tokens: 250,
            estimated_cost_usd: estimate_review_cost_usd(
                Some("codex"),
                Some("gpt-5.3-codex"),
                1800,
                250,
            ),
            pass1_candidate_findings: 2,
            confirmed_findings: 1,
            dismissed_findings: 1,
            false_positive_suppression_pct: 50,
            critical_count: 0,
            high_count: 1,
            medium_count: 0,
            low_count: 0,
            counts_by_category: cat_map,
            exact_anchor_count: 1,
            shifted_anchor_count: 0,
            unanchored_count: 0,
            exact_anchor_rate_pct: 100,
            coverage_rate_pct: Some(100),
            gate_failed: true,
            sarif_archive_path: None,
        };

        let rich_sarif = findings_to_sarif_json_with_telemetry(
            &confirmed,
            &dismissed_details,
            Some(&sample_telemetry),
        );
        assert_eq!(
            rich_sarif["runs"][0]["results"].as_array().unwrap().len(),
            2
        );
        assert_eq!(
            rich_sarif["runs"][0]["results"][0]["properties"]["disposition"],
            "confirmed"
        );
        assert_eq!(
            rich_sarif["runs"][0]["results"][1]["properties"]["disposition"],
            "dismissed_by_challenger"
        );
        assert_eq!(
            rich_sarif["runs"][0]["results"][1]["suppressions"][0]["justification"],
            "Caller already validates."
        );
        assert_eq!(
            rich_sarif["runs"][0]["properties"]["aidaTelemetry"]["ab_arm"],
            "ab-adversarial"
        );

        assert_eq!(
            resolve_ab_arm_assignment(Some("auto"), Some(100), None, "abc", true, false),
            ("ab-single-pass".to_string(), false)
        );
        assert_eq!(
            resolve_ab_arm_assignment(Some("auto"), Some(101), None, "abc", true, false),
            ("ab-adversarial".to_string(), true)
        );

        let report_md = format_review_telemetry_report_markdown(&[sample_telemetry]);
        assert!(report_md.contains("A/B Arm Summary"));
        assert!(report_md.contains("`ab-adversarial`"));
        assert!(report_md.contains("50% suppressed"));
        assert!(report_md.contains("`aida.review.fail-open`"));

        let reg = PlaybookRegistry::default();
        assert_eq!(reg.resolve("src/engine.cpp").id, "c_cpp");
        assert_eq!(reg.resolve("include/engine.hpp").id, "c_cpp");
        assert_eq!(reg.resolve("src/main/java/com/acme/Parser.java").id, "java");
        assert_eq!(reg.resolve("dsp/kalman_filter.m").id, "matlab");
        assert_eq!(reg.resolve("rtl/axi_fifo.sv").id, "hdl_rtl");
        assert_eq!(reg.resolve("rtl/uart_rx.vhd").id, "hdl_rtl");

        assert_eq!(
            canonical_module_stem("src/main/java/Parser.java"),
            canonical_module_stem("src/test/java/ParserTest.java")
        );
        assert_eq!(
            canonical_module_stem("rtl/axi_fifo.sv"),
            canonical_module_stem("tb/axi_fifo_tb.sv")
        );
        assert_eq!(canonical_module_stem("src/contest.rs"), "contest");
    }
}
