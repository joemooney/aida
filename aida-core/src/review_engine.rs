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

/// Lossless, injective text form of a raw git path: valid UTF-8 is kept,
/// `\` becomes `\\` and each byte outside valid UTF-8 becomes `\xHH`.
/// `decode_path` returns the exact bytes. trace:TASK-3869 | ai:claude
pub fn encode_path(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        out.push_str(&chunk.valid().replace('\\', "\\\\"));
        for b in chunk.invalid() {
            write!(out, "\\x{b:02X}").unwrap();
        }
    }
    out
}

/// The raw bytes `encode_path` encoded. A `\` starting no escape stays
/// literal, so a hand-written path such as `a\b` keeps its bytes.
// trace:TASK-3869 | ai:claude
pub fn decode_path(text: &str) -> Vec<u8> {
    let (bytes, mut out, mut i) = (text.as_bytes(), Vec::with_capacity(text.len()), 0);
    while i < bytes.len() {
        let hex = bytes
            .get(i + 2..i + 4)
            .and_then(|h| std::str::from_utf8(h).ok());
        let byte = hex.filter(|h| h.bytes().all(|c| c.is_ascii_hexdigit()));
        match (bytes[i], bytes.get(i + 1), byte) {
            (b'\\', Some(b'\\'), _) => (out.push(b'\\'), i += 2),
            (b'\\', Some(b'x'), Some(h)) => (out.push(u8::from_str_radix(h, 16).unwrap()), i += 4),
            (b, ..) => (out.push(b), i += 1),
        };
    }
    out
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
    /// Set when the entry is not a regular file; it is skipped, never read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub non_regular: Option<NonRegularKind>,
}

/// A git entry that is not a regular file. Review skips it and never follows
/// or reads through it. trace:TASK-3869 | ai:claude
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NonRegularKind {
    /// A symbolic link, or a path reached through a symlinked directory.
    Symlink,
    /// A gitlink: a submodule or nested repository.
    Gitlink,
    /// Any other non-regular entry (FIFO, socket, device).
    Other,
}

impl NonRegularKind {
    /// The kind for a git tree mode (`100644`, `120000`, `160000`, ...);
    /// `None` for a regular file or an absent side (`000000`).
    pub fn from_git_mode(mode: &str) -> Option<Self> {
        match mode {
            "100644" | "100755" | "100664" | "000000" => None,
            "120000" => Some(Self::Symlink),
            "160000" => Some(Self::Gitlink),
            _ => Some(Self::Other),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Symlink => "symlink",
            Self::Gitlink => "gitlink",
            Self::Other => "other",
        }
    }
}

/// Explicit reason why a changed path was excluded from active defect review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExclusionReason {
    NonRegular(NonRegularKind),
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
            Self::NonRegular(kind) => format!("non_regular:{}", kind.as_str()),
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
    /// Review-runner defaults; the CLI validates both against its allowlists
    /// and reads them only from the trusted base.
    // trace:STORY-3120 | ai:claude
    #[serde(default)]
    pub default_effort: Option<String>,
    #[serde(default)]
    pub default_model: Option<String>,
    /// Effort for the `--adversarial` challenger pass, validated the same way.
    #[serde(default)]
    pub challenger_effort: Option<String>,
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
    custom_rules: Vec<(CompiledGlob, CustomPathRule)>,
    builtin_rules: Vec<(CompiledGlob, ResolvedPlaybook)>,
    default_playbook: ResolvedPlaybook,
}

impl Default for PlaybookRegistry {
    fn default() -> Self {
        Self::new(Vec::new()).expect("built-in playbook globs compile")
    }
}

impl PlaybookRegistry {
    /// Construct a registry with built-in Rust-first + polyglot review playbooks
    /// layered beneath optional project-specific rules.
    pub fn new(custom_rules: Vec<CustomPathRule>) -> anyhow::Result<Self> {
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
            // trace:STORY-3120 | ai:claude
            ResolvedPlaybook {
                id: "c_cpp".to_string(),
                title: "C & C++ Memory Lifetime, Buffer/Integer UB & Thread Safety (CERT / C++ Core Guidelines)".to_string(),
                pattern: "**/*.{c,cc,cpp,cxx,h,hh,hpp,hxx}".to_string(),
                checklist: concat!(
                    "- **Memory lifetime & RAII**: Flag raw owning pointers, missing cleanup on error paths, use-after-free, double-free, and references, iterators or `std::string_view`/`std::span` outliving their storage.\n",
                    "- **Buffer bounds & integer UB**: Flag unchecked indexing, `memcpy`/`strcpy`/`sprintf` without bounds, signed overflow, shift-by-width, and uninitialized locals or struct fields.\n",
                    "- **Concurrency & contracts**: Verify lock ordering, atomic memory ordering, `errno`/return-code checks on OS calls, and header/implementation signature and `noexcept` parity."
                ).to_string(),
                active_facets: Vec::new(),
            },
            ResolvedPlaybook {
                id: "java_kotlin".to_string(),
                title: "Java & Kotlin Resource Lifecycle, Concurrency, Nullability & Injection Safety".to_string(),
                pattern: "**/*.{java,kt,kts}".to_string(),
                checklist: concat!(
                    "- **Resource lifecycle**: Ensure `AutoCloseable` streams, JDBC connections, statements and sockets close on every path (`try-with-resources` in Java, `use {}` in Kotlin).\n",
                    "- **Concurrency & equality**: Flag unsynchronized mutation of shared collections or fields, locks held while calling external code, and broken `equals`/`hashCode` contracts.\n",
                    "- **Nullability, exceptions & injection**: Flag swallowed exceptions, unchecked nulls (including Kotlin `!!` and platform types) across API boundaries, and unparameterized SQL, LDAP, command or deserialization inputs."
                ).to_string(),
                active_facets: Vec::new(),
            },
            ResolvedPlaybook {
                id: "matlab".to_string(),
                title: "MATLAB Matrix Dimensions, Numerical Conditioning & Workspace Hygiene".to_string(),
                pattern: "**/*.m".to_string(),
                checklist: concat!(
                    "- **Dimensions & 1-based indexing**: Verify row-vs-column orientation, unintended implicit expansion on mismatched dimensions, and off-by-one bounds under 1-based indexing.\n",
                    "- **Numerical conditioning**: Flag explicit inversion (`inv(A)*b` instead of `A\\b`), unguarded division by near-zero values, and silent `NaN`/`Inf` propagation.\n",
                    "- **Preallocation & workspace hygiene**: Flag arrays grown inside loops without preallocation, `eval`/`feval` on built strings, and variables shadowing built-in functions."
                ).to_string(),
                active_facets: Vec::new(),
            },
            ResolvedPlaybook {
                id: "hdl_rtl".to_string(),
                title: "Verilog, SystemVerilog & VHDL RTL Latches, CDC, Reset & Width Safety".to_string(),
                pattern: "**/*.{v,vh,sv,svh,vhd,vhdl}".to_string(),
                checklist: concat!(
                    "- **Inferred latches**: Flag `always @(*)`, `always_comb` or combinational `process` blocks whose `if`/`case` paths leave an output unassigned (no default).\n",
                    "- **Assignments & drivers**: Verify clocked logic uses non-blocking `<=` and combinational logic blocking `=`, and flag signals driven from more than one `always`/`process` block.\n",
                    "- **CDC, resets & widths**: Flag signals crossing asynchronous clock domains without a synchronizer or FIFO, registers missing reset initialization, and silent width truncation or signed/unsigned mismatch."
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

        Ok(Self {
            custom_rules: custom_rules
                .into_iter()
                .map(|rule| {
                    let glob = CompiledGlob::new(&rule.pattern).with_context(|| {
                        format!("invalid playbook {} pattern {:?}", rule.id, rule.pattern)
                    })?;
                    Ok((glob, rule))
                })
                .collect::<anyhow::Result<_>>()?,
            builtin_rules: builtin_rules
                .into_iter()
                .map(|rule| {
                    let glob = CompiledGlob::new(&rule.pattern).with_context(|| {
                        format!("invalid playbook {} pattern {:?}", rule.id, rule.pattern)
                    })?;
                    Ok((glob, rule))
                })
                .collect::<anyhow::Result<_>>()?,
            default_playbook,
        })
    }

    /// Resolve the effective base playbook for a relative repository path.
    pub fn resolve(&self, rel_path: &str) -> ResolvedPlaybook {
        let builtin = self
            .builtin_rules
            .iter()
            .find(|(glob, _)| glob.matches(rel_path))
            .map(|(_, rule)| rule)
            .unwrap_or(&self.default_playbook);

        for (index, (glob, custom)) in self.custom_rules.iter().enumerate() {
            if glob.matches(rel_path) {
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
                        id: format!("rule:{index}:{}", custom.id),
                        title: format!("Project Rule ({})", custom.id),
                        pattern: custom.pattern.clone(),
                        checklist: custom.checklist.trim().to_string(),
                        active_facets: vec![custom.id.clone()],
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
        corpus_samples: &[(&str, &str)],
    ) -> ResolvedPlaybook {
        // trace:TASK-3880 | ai:codex
        let rust_samples: Vec<_> = corpus_samples
            .iter()
            .filter_map(|&(path, body)| {
                Path::new(path)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("rs"))
                    .then_some(body)
            })
            .collect();
        if rust_samples.is_empty() {
            return playbook;
        }
        let facets = detect_rust_content_facets(&rust_samples);
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
    CompiledGlob::new(pattern).is_ok_and(|glob| glob.matches(candidate))
}

// trace:TASK-3872 | ai:codex
#[derive(Debug, Clone)]
pub struct CompiledGlob(Regex);

#[cfg(test)]
thread_local! { static GLOB_COMPILES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

impl CompiledGlob {
    fn new(pattern: &str) -> anyhow::Result<Self> {
        #[cfg(test)]
        GLOB_COMPILES.set(GLOB_COMPILES.get() + 1);
        let alternatives = expand_braces(pattern)
            .into_iter()
            .map(|pat| glob_to_regex(&pat.replace('\\', "/").to_ascii_lowercase()))
            .collect::<Vec<_>>();
        if alternatives.is_empty() {
            anyhow::bail!("brace expansion exceeds compilation limits");
        }
        Ok(Self(Regex::new(&alternatives.join("|"))?))
    }

    fn matches(&self, candidate: &str) -> bool {
        self.0
            .is_match(&candidate.replace('\\', "/").to_ascii_lowercase())
    }
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
    let count = pattern[open + 1..close].split(',').count();
    if count > 256
        || (prefix.len() + suffix.len())
            .saturating_mul(count)
            .saturating_add(close - open)
            > 1024 * 1024
    {
        return Vec::new();
    }
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
) -> anyhow::Result<Option<ExclusionReason>> {
    Ok(classify_compiled(
        entry,
        &compile_exclusions(user_excludes)?,
    ))
}

pub fn compile_exclusions(patterns: &[String]) -> anyhow::Result<Vec<(&str, CompiledGlob)>> {
    patterns
        .iter()
        .map(|p| {
            Ok((
                p.as_str(),
                CompiledGlob::new(p).with_context(|| format!("invalid exclusion pattern {p:?}"))?,
            ))
        })
        .collect()
}

pub fn classify_compiled(
    entry: &RawDiffEntry,
    user_excludes: &[(&str, CompiledGlob)],
) -> Option<ExclusionReason> {
    if entry.is_binary {
        return Some(ExclusionReason::BinaryFile);
    }
    if entry.kind == FileChangeKind::Deleted {
        return Some(ExclusionReason::PureDeletion);
    }

    // trace:TASK-3866 | ai:codex
    // A path reason outranks the non-regular label, so a secret-named
    // symlink or FIFO still reads `secret_sensitive_path`; either way the
    // entry is excluded and never read. trace:TASK-3869 | ai:claude
    std::iter::once(entry.path.as_str())
        .chain(entry.old_path.as_deref())
        .find_map(|path| classify_path_exclusion(path, user_excludes))
        .or(entry.non_regular.map(ExclusionReason::NonRegular))
}

/// The exclusion `path` alone hits: secret, substrate, generated or user
/// pattern. A rename's old path must pass it too.
// trace:STORY-3120 | ai:claude
pub fn classify_path_exclusion(
    path: &str,
    user_excludes: &[(&str, CompiledGlob)],
) -> Option<ExclusionReason> {
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
    for (pat, glob) in user_excludes {
        if glob.matches(&norm) {
            return Some(ExclusionReason::UserExcludedPattern((*pat).to_owned()));
        }
    }

    None
}

/// True when `path` (repository-relative, `/` or `\` separated) names a
/// credential file or lies under a credential directory, at the repository
/// root or nested at any depth. Such paths are never read into a review
/// prompt, and findings that cite them are refused.
// trace:TASK-3320 | ai:claude
pub fn is_credential_path(path: &str) -> bool {
    let norm = path.replace('\\', "/");
    let file_name = norm.rsplit('/').next().unwrap_or(&norm);
    is_secret_sensitive_path(&norm, file_name)
}

fn is_secret_sensitive_path(norm_path: &str, file_name: &str) -> bool {
    let lower_name = file_name.to_ascii_lowercase();
    if lower_name == ".env"
        || (lower_name.starts_with(".env.") && lower_name != ".env.example")
        || lower_name.ends_with(".pem")
        || lower_name.ends_with(".key")
        || lower_name.ends_with(".p12")
        || lower_name.ends_with(".pfx")
        || matches!(
            lower_name.as_str(),
            "id_rsa"
                | "id_dsa"
                | "id_ecdsa"
                | "id_ed25519"
                | "credentials.json"
                | ".netrc"
                | "_netrc"
                | ".git-credentials"
                | ".pgpass"
                | ".pypirc"
                // trace:TASK-3914 | ai:claude
                | ".npmrc"
                | ".vault-token"
                | ".terraformrc"
                | "terraform.rc"
                | ".dockercfg"
        )
    {
        return true;
    }
    // trace:TASK-3320 | ai:claude
    // Component matching, ASCII case-insensitive (a case-insensitive checkout
    // resolves `.AWS/credentials` to the same directory as `.aws/credentials`).
    let parts: Vec<String> = norm_path
        .split('/')
        .map(|part| part.to_ascii_lowercase())
        .collect();
    parts
        .iter()
        .any(|part| matches!(part.as_str(), ".aws" | ".ssh" | ".gnupg" | ".azure"))
        || parts.windows(2).any(|w| {
            matches!(
                (w[0].as_str(), w[1].as_str()),
                (".config", "gcloud")
                    | (".config", "gh")
                    | (".docker", "config.json")
                    | (".kube", "config")
                    // trace:TASK-3914 | ai:claude
                    | (".config", "hub")
                    | (".cargo", "credentials")
                    | (".cargo", "credentials.toml")
                    | (".terraform.d", "credentials.tfrc.json")
                    | (".gem", "credentials")
            )
        })
}

/// Classify raw diff entries into reviewable and excluded lists, resolving each
/// reviewable file's playbook ID and computing a canonical SHA-256 inventory digest.
// trace:STORY-3076 | ai:antigravity
pub fn classify_diff_inventory<F>(
    entries: &[RawDiffEntry],
    registry: &PlaybookRegistry,
    user_excludes: &[String],
    token_cap: usize,
    trace_detector: F,
) -> anyhow::Result<ClassifiedInventory>
where
    F: FnMut(&str) -> bool,
{
    let excludes = compile_exclusions(user_excludes)?;
    Ok(classify_with_exclusions(
        entries,
        registry,
        &excludes,
        token_cap,
        trace_detector,
    ))
}

pub fn classify_with_exclusions(
    entries: &[RawDiffEntry],
    registry: &PlaybookRegistry,
    excludes: &[(&str, CompiledGlob)],
    token_cap: usize,
    mut trace_detector: impl FnMut(&str) -> bool,
) -> ClassifiedInventory {
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
        let reason = classify_compiled(entry, excludes)
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
/// Keep repository directories case-sensitive when identifying companions.
// trace:TASK-3880 | ai:codex
pub fn canonical_module_stem(path: &str) -> String {
    let raw_name = Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path);
    let raw_stem = match raw_name.rfind('.') {
        Some(idx) if idx > 0 => &raw_name[..idx],
        _ => raw_name,
    };
    // JUnit-style `FooTest`/`FooTests` pair with `Foo`. Case-sensitive, before
    // lowercasing, so `contest`/`Latest` keep their stems. trace:STORY-3120 | ai:claude
    let raw_stem = raw_stem
        .strip_suffix("Tests")
        .or_else(|| raw_stem.strip_suffix("Test"))
        .filter(|prefix| {
            prefix.starts_with(|c: char| c.is_ascii_uppercase())
                && prefix.ends_with(|c: char| c.is_ascii_alphanumeric())
        })
        .unwrap_or(raw_stem);
    let lowered = raw_stem.to_ascii_lowercase();
    let without_ext = lowered.as_str();

    let mut core_stem = without_ext;
    // `_tb` / `tb_` are HDL testbench conventions. trace:STORY-3120 | ai:claude
    for suffix in ["_tests", "_test", ".test", ".spec", "_tb"] {
        if let Some(stripped) = without_ext.strip_suffix(suffix) {
            if !stripped.is_empty() {
                core_stem = stripped;
                break;
            }
        }
    }
    if core_stem == without_ext {
        if let Some(stripped) = ["test_", "tb_"]
            .iter()
            .find_map(|prefix| without_ext.strip_prefix(prefix))
            .filter(|stripped| !stripped.is_empty())
        {
            core_stem = stripped;
        }
    }

    let mut dir = Path::new(path).parent().unwrap_or(Path::new(""));
    if dir.ends_with("tests") || dir.ends_with("test") {
        dir = dir.parent().unwrap_or(dir);
    }
    // Maven/Gradle: `src/test/<lang>/…` mirrors `src/main/<lang>/…`.
    // trace:STORY-3120 | ai:claude
    let parts: Vec<_> = dir.components().collect();
    let mirrored = parts
        .windows(2)
        .position(|w| w[0].as_os_str() == "src" && w[1].as_os_str() == "test");
    let dir = match mirrored {
        Some(at) => parts
            .iter()
            .enumerate()
            .map(|(i, c)| {
                if i == at + 1 {
                    std::ffi::OsStr::new("main")
                } else {
                    c.as_os_str()
                }
            })
            .collect::<std::path::PathBuf>(),
        None => dir.to_path_buf(),
    };
    dir.join(core_stem).to_string_lossy().into_owned()
}

/// Bundle reviewable files into semantic, token-bounded batches:
/// 1. Files sharing a canonical module stem (e.g. `foo.rs` and `foo_test.rs`)
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
    let keys: BTreeSet<_> = files
        .iter()
        .map(|f| canonical_module_stem(&f.path))
        .collect();
    for f in files {
        let mut key = canonical_module_stem(&f.path);
        if matches!(
            Path::new(&f.path).parent().and_then(Path::to_str),
            Some("tests" | "test")
        ) && keys.contains(&format!("src/{key}"))
        {
            key = format!("src/{key}");
        }
        by_stem.entry(key).or_default().push(f);
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
        let mut current_stems = BTreeMap::new();
        let mut part = 1usize;

        for f in group {
            let would_exceed_files = current.len() >= file_cap;
            let would_exceed_tokens = !current.is_empty()
                && f.estimated_tokens > token_cap.saturating_sub(current_tokens);
            let key = canonical_module_stem(&f.path);
            let (dir, stem) = key.rsplit_once('/').unwrap_or(("", &key));
            let other_directory = current_stems.get(stem).is_some_and(|seen| seen != dir);
            if would_exceed_files || would_exceed_tokens || other_directory {
                raw_batches.push((
                    format!("playbook:{playbook_id}#{}", part),
                    std::mem::take(&mut current),
                ));
                current_tokens = 0;
                current_stems.clear();
                part += 1;
            }
            current_tokens = current_tokens.saturating_add(f.estimated_tokens);
            current_stems.insert(stem.to_owned(), dir.to_owned());
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
    let snippet_trimmed = snippet.trim_matches(['\r', '\n']);
    if snippet_trimmed.trim().is_empty() {
        return None;
    }

    let file_lines: Vec<&str> = file_content.lines().collect();
    let needle_lines: Vec<&str> = snippet_trimmed.lines().collect();
    if needle_lines.is_empty() || needle_lines.len() > file_lines.len() {
        return None;
    }

    // Pass 1: exact line-by-line match.
    let exact_candidates = matching_spans(&needle_lines, &file_lines, |s| s.trim_end_matches('\r'));

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

    let trimmed_candidates = matching_spans(&norm_needle, &file_lines, str::trim);

    pick_closest_span(&trimmed_candidates, hint_line).map(|(start_line, end_line)| AnchoredSpan {
        start_line,
        end_line,
        match_kind: "trimmed_whitespace".to_string(),
    })
}

// trace:TASK-3872 | ai:codex
#[cfg(test)]
thread_local! { static LINE_COMPARISONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

fn matching_spans(
    needle: &[&str],
    lines: &[&str],
    normalize: fn(&str) -> &str,
) -> Vec<(usize, usize)> {
    let needle: Vec<_> = needle.iter().map(|line| normalize(line)).collect();
    let mut prefix = vec![0; needle.len()];
    let advance = |mut n: usize, line: &str, prefix: &[usize]| {
        let equals = |n: usize| {
            #[cfg(test)]
            LINE_COMPARISONS.set(LINE_COMPARISONS.get() + 1);
            needle[n] == line
        };
        while n > 0 && !equals(n) {
            n = prefix[n - 1];
        }
        n + usize::from(equals(n))
    };
    for i in 1..needle.len() {
        prefix[i] = advance(prefix[i - 1], needle[i], &prefix);
    }
    let mut spans = Vec::new();
    let mut matched = 0;
    for (i, line) in lines.iter().enumerate() {
        matched = advance(matched, normalize(line), &prefix);
        if matched == needle.len() {
            spans.push((i + 2 - matched, i + 1));
            matched = prefix[matched - 1];
        }
    }
    spans
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
    pub resolved_span: Option<AnchoredSpan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_status: Option<String>,
}

/// Anchor a batch of findings against a file-content provider, updating `resolved_span` and `anchor_status` (`"exact"`, `"trimmed_whitespace"`, `"unanchored"`, or `"no_snippet"`).
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
        cloned.resolved_span = None;
        if let Some(snippet) = f.existing_code.as_deref() {
            if let Some(content) = read_file(&f.path) {
                if let Some(span) = anchor_snippet_lines(&content, snippet, f.start_line) {
                    cloned.anchor_status = Some(span.match_kind.clone());
                    cloned.resolved_span = Some(span);
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

/// Convert anchored review findings into a SARIF 2.1.0 JSON document suitable for
/// GitHub Code Scanning (`github/codeql-action/upload-sarif`).
// trace:STORY-3120 | ai:codex
pub fn findings_to_sarif_json(findings: &[RawReviewFinding]) -> serde_json::Value {
    findings_to_sarif_json_with_suppressed(findings, &[])
}

/// As [`findings_to_sarif_json`], plus `suppressed` findings, each with its
/// justification, emitted as SARIF results carrying an `external` suppression
/// rather than dropped.
// trace:STORY-3120 | ai:claude
pub fn findings_to_sarif_json_with_suppressed(
    findings: &[RawReviewFinding],
    suppressed: &[(RawReviewFinding, String)],
) -> serde_json::Value {
    let mut rules_map: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    let mut results: Vec<serde_json::Value> = Vec::with_capacity(findings.len());
    let rows = (findings.iter().map(|f| (f, None)))
        .chain(suppressed.iter().map(|(f, why)| (f, Some(why))));

    for (f, justification) in rows {
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

        let level = match severity_rank(&f.severity) {
            Some(3..=4) | None => "error",
            Some(2) => "warning",
            _ => "note",
        };

        let (start, end) = match &f.resolved_span {
            Some(span) => (span.start_line, span.end_line),
            None => (f.start_line.unwrap_or(1), f.end_line.unwrap_or(1)),
        };
        let start_line = start.max(1);
        let end_line = end.max(start_line);

        // Encode path bytes so reserved characters remain part of the filename;
        // the raw bytes, not their text form. trace:TASK-3869 | ai:claude
        let mut uri = String::new();
        for byte in decode_path(&f.path) {
            if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
                uri.push(char::from(byte));
            } else {
                use std::fmt::Write;
                write!(uri, "%{byte:02X}").unwrap();
            }
        }

        let mut message_text = f.message.clone();
        if let Some(suggestion) = f.suggestion_code.as_deref() {
            if !suggestion.trim().is_empty() {
                message_text.push_str("\n\nSuggested fix:\n");
                message_text.push_str(suggestion.trim());
            }
        }

        // SARIF 2.1 §3.11.5 requires doubling literal braces in message strings.
        let message_text = message_text.replace('{', "{{").replace('}', "}}");
        let mut result = serde_json::json!({
            "ruleId": rule_id,
            "level": level,
            "message": {
                "text": message_text
            },
            "locations": [
                {
                    "physicalLocation": {
                        "artifactLocation": {
                            "uri": uri,
                            "uriBaseId": "%SRCROOT%"
                        },
                        "region": {
                            "startLine": start_line,
                            "endLine": end_line
                        }
                    }
                }
            ]
        });
        if let Some(why) = justification {
            result["suppressions"] =
                serde_json::json!([{"kind": "external", "justification": why}]);
        }
        results.push(result);
    }

    let rules: Vec<serde_json::Value> = rules_map.into_values().collect();
    serde_json::json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [
            {
                "tool": {
                    "driver": {
                        "name": "aida-review-engine",
                        "informationUri": "https://github.com/joemooney/aida",
                        "rules": rules
                    }
                },
                "originalUriBaseIds": {
                    "%SRCROOT%": {"description": {"text": "Repository root"}}
                },
                "results": results
            }
        ]
    })
}

/// Envelope parsed from an automated batch review runner's stdout.
#[derive(Deserialize)]
struct BatchRunnerOutput {
    findings: Vec<RawReviewFinding>,
    #[serde(default)]
    files: Vec<FileCoverageEntry>,
}

/// Parse JSON output from an automated batch review runner (`claude --print` or
/// `AIDA_REVIEW_RUNNER_CMD`). Accepts either a `{ "findings": [...], "files": [...] }`
/// object or a bare `[ RawReviewFinding, ... ]` array, stripping surrounding markdown
/// code fences if present.
// trace:STORY-3120 | ai:codex
pub fn parse_runner_findings_json(
    raw: &str,
) -> Result<(Vec<RawReviewFinding>, CoverageLedger), String> {
    let raw = raw.trim();
    let trimmed = if let Some(fenced) = raw.strip_prefix("```") {
        let (language, body) = fenced.split_once('\n').ok_or("missing fenced JSON body")?;
        if !language.trim().is_empty() && !language.trim().eq_ignore_ascii_case("json") {
            return Err("expected a JSON code fence".into());
        }
        body.trim_end()
            .strip_suffix("```")
            .ok_or("unterminated JSON fence")?
            .trim()
    } else {
        raw
    };
    if trimmed.starts_with('[') {
        let findings: Vec<RawReviewFinding> = serde_json::from_str(trimmed)
            .map_err(|e| format!("failed to parse runner findings array: {e}"))?;
        return Ok((findings, CoverageLedger::default()));
    }
    let env: BatchRunnerOutput = serde_json::from_str(trimmed)
        .map_err(|e| format!("failed to parse runner output object: {e}"))?;
    Ok((env.findings, CoverageLedger { files: env.files }))
}

/// Numeric rank for a review finding severity (`critical`=4, `high`=3, `medium`=2, `low`=1).
// trace:STORY-3120 | ai:codex
pub fn severity_rank(severity: &str) -> Option<u8> {
    match severity.trim().to_ascii_lowercase().as_str() {
        "critical" => Some(4),
        "high" | "error" => Some(3),
        "medium" | "warning" | "warn" => Some(2),
        "low" | "info" | "note" => Some(1),
        _ => None,
    }
}

/// Return all findings whose severity meets or exceeds `threshold` (`critical`, `high`, `medium`, `low`).
// trace:STORY-3120 | ai:codex
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
        .filter(|f| severity_rank(&f.severity).unwrap_or(4) >= min_rank)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    // trace:STORY-3120 | ai:codex
    #[test]
    fn s1_runner_findings_sarif_and_threshold() {
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

        let exceeded = findings_exceeding_threshold(&findings, "high").unwrap();
        assert_eq!(exceeded.len(), 1);
        assert!(findings_exceeding_threshold(&findings, "critical")
            .unwrap()
            .is_empty());
        let mut moved = findings[0].clone();
        moved.severity = " ERROR ".into();
        moved.path = r"src/a\b #%.rs".into();
        moved.message = "use {0}".into();
        moved.suggestion_code = Some("if ok {}".into());
        moved.resolved_span = Some(AnchoredSpan {
            start_line: 8,
            end_line: 10,
            match_kind: "exact".into(),
        });
        let both = [findings[0].clone(), moved];
        let run = &findings_to_sarif_json(&both)["runs"][0];
        assert_eq!(run["tool"]["driver"]["rules"].as_array().unwrap().len(), 1);
        let location = &run["results"][1]["locations"][0]["physicalLocation"];
        assert_eq!(
            location["region"],
            serde_json::json!({"startLine":8,"endLine":10})
        );
        assert_eq!(location["artifactLocation"]["uri"], "src/a%5Cb%20%23%25.rs");
        let base_id = location["artifactLocation"]["uriBaseId"].as_str().unwrap();
        assert_eq!(base_id, "%SRCROOT%");
        assert!(run["originalUriBaseIds"][base_id].is_object());
        assert_eq!(
            run["results"][1]["message"]["text"],
            "use {{0}}\n\nSuggested fix:\nif ok {{}}"
        );
        assert_eq!(
            findings_exceeding_threshold(&both, " HIGH ").unwrap().len(),
            2
        );
        assert!(findings_exceeding_threshold(&both, "other").is_err());
        for severity in ["blocker", ""] {
            let mut unknown = findings[0].clone();
            unknown.severity = severity.into();
            let report = findings_to_sarif_json(std::slice::from_ref(&unknown));
            assert_eq!(report["runs"][0]["results"][0]["level"], "error");
            assert_eq!(
                findings_exceeding_threshold(&[unknown], "critical")
                    .unwrap()
                    .len(),
                1
            );
        }
        assert!(parse_runner_findings_json("[]").unwrap().0.is_empty());
        assert!(parse_runner_findings_json(r#"{"error":"runner failed"}"#).is_err());
    }

    // trace:TASK-3872 | ai:codex
    /// Raw path bytes survive the text form exactly, and distinct byte
    /// strings never share one. trace:TASK-3869 | ai:claude
    #[test]
    fn t3869_path_text_form_is_lossless_and_injective() {
        let cases: [&[u8]; 6] = [
            b"caf\xe9.rs",
            br"caf\xE9.rs",
            br"a\\b",
            "naïve.rs".as_bytes(),
            b"\xff\\x",
            b"",
        ];
        let encoded: BTreeSet<String> = cases.iter().map(|c| encode_path(c)).collect();
        assert_eq!(encoded.len(), cases.len());
        for c in cases {
            assert_eq!(decode_path(&encode_path(c)), c);
        }
        assert_eq!(encode_path(b"caf\xe9.rs"), r"caf\xE9.rs");
        assert_eq!(encode_path("naïve.rs".as_bytes()), "naïve.rs");
    }

    #[test]
    fn h2b_compiled_globs_bound_expansion() {
        let rules = [String::from("*{*,x}/FoO")];
        let registry = PlaybookRegistry::default();
        let compiled = compile_exclusions(&rules).unwrap();
        let before = GLOB_COMPILES.get();
        for path in ["one.rs", "two.rs"] {
            registry.resolve(path);
        }
        assert!(compiled[0].1.matches(r"a\b\foo"));
        assert!(compiled[0].1.matches("foo"));
        assert_eq!(GLOB_COMPILES.get(), before);
        let invalid = format!("{{{}z}}", "x,".repeat(256));
        let error = classify_diff_inventory(
            &diff_entries(&["ordinary.rs"], false),
            &registry,
            std::slice::from_ref(&invalid),
            usize::MAX,
            |_| panic!("classified invalid inventory"),
        )
        .unwrap_err();
        assert!(error.to_string().contains(&invalid));
    }

    #[test]
    fn h2b_crlf_padding() {
        let span = anchor_snippet_lines("a\nb", "\r\n\r\na\r\nb\r\n\r\n", None).unwrap();
        assert_eq!(
            (span.start_line, span.end_line, span.match_kind.as_str()),
            (1, 2, "exact")
        );
    }

    #[test]
    fn h2b_resolved_span_preserves_hints() {
        let finding = serde_json::from_value(serde_json::json!({"path":"a","severity":"low","category":"c","message":"m","existing_code":"x","start_line":9,"end_line":10})).unwrap();
        let anchored = anchor_findings_with(&[finding], |_| Some("x\nx".into()));
        assert_eq!(
            (anchored[0].start_line, anchored[0].end_line),
            (Some(9), Some(10))
        );
        assert_eq!(anchored[0].resolved_span.as_ref().unwrap().start_line, 2);
        assert!(anchor_findings_with(&anchored, |_| None)[0]
            .resolved_span
            .is_none());
    }

    #[test]
    fn h2b_linear_repeated_prefix() {
        LINE_COMPARISONS.set(0);
        let file = format!("{}b", "a\n".repeat(32000));
        let snippet = format!("{}b", "a\n".repeat(16000));
        assert_eq!(
            anchor_snippet_lines(&file, &snippet, None)
                .unwrap()
                .start_line,
            16001
        );
        assert!((1..=3 * (32001 + 16001)).contains(&LINE_COMPARISONS.get()));
    }

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
                non_regular: None,
                is_binary: binary,
            })
            .collect()
    }

    fn inventory(paths: &[&str], binary: bool) -> ClassifiedInventory {
        let entries = diff_entries(paths, binary);
        classify_diff_inventory(&entries, &Default::default(), &[], usize::MAX, |_| false).unwrap()
    }

    #[test]
    fn h2a_digest_frames_paths() {
        let pair = inventory(&["a", "b"], true).inventory_sha256;
        assert_eq!(
            pair,
            "820722b38f0f59d7c5bdbe39d59476b82e641280521335f50b4b359fd5f024e5"
        );
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
        let inv =
            classify_diff_inventory(&entries, &Default::default(), &[], 32, |_| false).unwrap();
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

    // trace:TASK-3320 | ai:claude
    #[test]
    fn root_and_nested_credential_paths_are_excluded() {
        for rel in [
            ".aws/credentials",
            ".ssh/config",
            ".gnupg/pubring.kbx",
            ".azure/accessTokens.json",
            ".config/gcloud/application_default_credentials.json",
            ".config/gh/hosts.yml",
            ".docker/config.json",
            ".kube/config",
            ".netrc",
            "_netrc",
            ".git-credentials",
            ".pgpass",
            ".pypirc",
            ".AWS/credentials",
            ".Docker/Config.json",
        ] {
            for prefix in ["", "nested/", "a/b/"] {
                let path = format!("{prefix}{rel}");
                assert_eq!(
                    classify_path_exclusion(&path, &[]),
                    Some(ExclusionReason::SecretSensitivePath),
                    "{path}"
                );
                assert!(is_credential_path(&path), "{path}");
                assert!(is_credential_path(&path.replace('/', "\\")), "{path}");
            }
        }
        for path in [
            ".docker/Dockerfile",
            ".config/nvim/init.lua",
            "gcloud/setup.md",
            "src/netrc.rs",
            ".kube-docs/config",
        ] {
            assert_eq!(classify_path_exclusion(path, &[]), None, "{path}");
            assert!(!is_credential_path(path), "{path}");
        }
    }

    // trace:TASK-3914 | ai:claude
    fn assert_credential_at_every_depth(rel: &str) {
        for prefix in ["", "nested/", "a/b/"] {
            for path in [
                format!("{prefix}{rel}"),
                format!("{prefix}{}", rel.to_ascii_uppercase()),
            ] {
                assert_eq!(
                    classify_path_exclusion(&path, &[]),
                    Some(ExclusionReason::SecretSensitivePath),
                    "{path}"
                );
                assert!(is_credential_path(&path), "{path}");
                assert!(is_credential_path(&path.replace('/', "\\")), "{path}");
            }
        }
    }

    #[test]
    fn credential_npmrc() {
        assert_credential_at_every_depth(".npmrc");
    }

    #[test]
    fn credential_cargo_credentials() {
        assert_credential_at_every_depth(".cargo/credentials");
    }

    #[test]
    fn credential_cargo_credentials_toml() {
        assert_credential_at_every_depth(".cargo/credentials.toml");
    }

    #[test]
    fn credential_vault_token() {
        assert_credential_at_every_depth(".vault-token");
    }

    #[test]
    fn credential_terraformrc() {
        assert_credential_at_every_depth(".terraformrc");
    }

    #[test]
    fn credential_terraform_rc() {
        assert_credential_at_every_depth("terraform.rc");
    }

    #[test]
    fn credential_terraform_d_tfrc() {
        assert_credential_at_every_depth(".terraform.d/credentials.tfrc.json");
    }

    #[test]
    fn credential_gem_credentials() {
        assert_credential_at_every_depth(".gem/credentials");
    }

    #[test]
    fn credential_dockercfg() {
        assert_credential_at_every_depth(".dockercfg");
    }

    #[test]
    fn credential_config_hub() {
        assert_credential_at_every_depth(".config/hub");
    }

    #[test]
    fn credential_pattern_neighbours_stay_reviewable() {
        for path in [
            ".cargo/config",
            "nested/.cargo/env",
            "docs/npmrc.md",
            "terraform/main.tf",
            ".terraform.d/plugins/x",
            ".gem/specs/x.gemspec",
            ".config/hubble.toml",
            "vault-token.rs",
        ] {
            assert_eq!(classify_path_exclusion(path, &[]), None, "{path}");
            assert!(!is_credential_path(path), "{path}");
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
                non_regular: None,
            };
            assert_eq!(
                classify_exclusion(&entry, &[]).unwrap(),
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
                non_regular: None,
            },
            RawDiffEntry {
                path: "Cargo.lock".to_string(),
                old_path: None,
                kind: FileChangeKind::Modified,
                additions: 40,
                deletions: 10,
                is_binary: false,
                patch_bytes: 1600,
                non_regular: None,
            },
            RawDiffEntry {
                path: ".env.production".to_string(),
                old_path: None,
                kind: FileChangeKind::Added,
                additions: 5,
                deletions: 0,
                is_binary: false,
                patch_bytes: 200,
                non_regular: None,
            },
            RawDiffEntry {
                path: "old_deprecated.rs".to_string(),
                old_path: None,
                kind: FileChangeKind::Deleted,
                additions: 0,
                deletions: 80,
                is_binary: false,
                patch_bytes: 2000,
                non_regular: None,
            },
            RawDiffEntry {
                path: "docs/legacy/notes.md".to_string(),
                old_path: None,
                kind: FileChangeKind::Modified,
                additions: 10,
                deletions: 2,
                is_binary: false,
                patch_bytes: 400,
                non_regular: None,
            },
        ];

        let user_excludes = vec!["docs/legacy/**".to_string()];
        let inv =
            classify_diff_inventory(&entries, &registry, &user_excludes, usize::MAX, |path| {
                path.ends_with("review_engine.rs")
            })
            .unwrap();

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
                id: "rust".to_string(),
                pattern: "**/*.proto".to_string(),
                checklist: "- Ensure field numbers are never reused.".to_string(),
                merge_builtin: false,
            },
        ];
        let reg = PlaybookRegistry::new(custom).unwrap();

        let merged = reg.resolve("aida-cli-lib/src/gate/review_verdict.rs");
        assert_eq!(merged.id, "rust+gate_strict");
        assert!(merged.checklist.contains("Flag any `.unwrap()`"));
        assert!(merged
            .checklist
            .contains("Verify gate authority never fails open."));

        let replaced = reg.resolve("proto/aida.proto");
        let unrelated = reg.resolve("other.rs");
        assert_ne!(replaced.id, unrelated.id);
        assert_eq!(replaced.active_facets, ["rust"]);
        assert!(unrelated.active_facets.is_empty());
        assert!(!unrelated.checklist.contains("field numbers"));
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
            &[(
                "pure_math.rs",
                "pub fn add(a: i32, b: i32) -> i32 { a + b }",
            )],
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
        let enriched = reg.enrich_with_content_facets(base, &[("src/run.rs", io_unsafe_sample)]);
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

    // trace:TASK-b6b1278890774ef993d2ebc327862470-1364.AC2 | ai:codex
    #[test]
    fn h2c_rust_facets_follow_sample_paths() {
        let reg = PlaybookRegistry::default();
        let mut pb = reg.resolve("a.rs");
        pb.id = "custom".into();
        let samples = [("a.Rs", "unsafe {"), ("a.py", "Command::new(")];
        let enriched = reg.enrich_with_content_facets(pb.clone(), &samples);
        assert_eq!(enriched.active_facets, ["rust_unsafe_ffi"]);
        pb.id = "rust-custom".into();
        assert!(reg
            .enrich_with_content_facets(pb, &samples[1..])
            .active_facets
            .is_empty());
    }

    // trace:TASK-b6b1278890774ef993d2ebc327862470-1364.AC1 | ai:codex
    #[test]
    fn h2c_equal_stems_in_different_directories_stay_separate() {
        assert_eq!(
            canonical_module_stem("src/foo.rs"),
            canonical_module_stem("src/tests/foo_tests.rs")
        );
        let files = inventory(
            &["A/src/file.rs", "a/src/file_test.rs", "a/tests/file.rs"],
            false,
        )
        .reviewable_files;
        let batches = bundle_review_batches(&files, usize::MAX, 10).unwrap();
        assert_eq!(batches.len(), 3);
        assert!(batches.iter().all(|batch| batch.files.len() == 1));
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

        // Budget 2000 tokens: parser.rs + parser_test.rs (1400 tokens) pair into `module:src/parser`,
        // while alpha.rs (1500) and beta.rs (1500) split into two separate playbook batches.
        let batches = bundle_review_batches(&files, 2000, 4).unwrap();
        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0].label, "module:src/parser");
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

    // trace:STORY-3120 | ai:claude
    #[test]
    fn s6_polyglot_and_hdl_playbooks_match_and_carry_checklists() {
        let reg = PlaybookRegistry::default();
        let cases: &[(&str, &[&str], &[&str])] = &[
            (
                "c_cpp",
                &[
                    "src/engine.c",
                    "src/engine.cpp",
                    "lib/a.cc",
                    "x.cxx",
                    "include/engine.h",
                    "include/engine.hpp",
                    "a.hh",
                    "a.hxx",
                ],
                &["use-after-free", "memcpy", "noexcept"],
            ),
            (
                "java_kotlin",
                &[
                    "src/main/java/com/acme/Parser.java",
                    "app/src/Main.kt",
                    "build.gradle.kts",
                ],
                &["try-with-resources", "use {}", "hashCode", "!!"],
            ),
            (
                "matlab",
                &["dsp/kalman_filter.m"],
                &["A\\b", "1-based", "preallocation"],
            ),
            (
                "hdl_rtl",
                &[
                    "rtl/axi_fifo.v",
                    "rtl/defs.vh",
                    "rtl/axi_fifo.sv",
                    "rtl/pkg.svh",
                    "rtl/uart_rx.vhd",
                    "rtl/uart_tx.vhdl",
                ],
                &["always_comb", "<=", "synchronizer"],
            ),
        ];
        for (id, paths, needles) in cases {
            for path in *paths {
                let pb = reg.resolve(path);
                assert_eq!(pb.id, *id, "{path}");
                for needle in *needles {
                    assert!(pb.checklist.contains(needle), "{id} lacks {needle:?}");
                }
            }
        }
        // Lookalikes stay on their own or the general playbook.
        for (path, id) in [
            ("a.mk", "general"),
            ("a.vue", "general"),
            ("a.svg", "general"),
            ("a.json", "config_data"),
            ("a.hs", "general"),
        ] {
            assert_eq!(reg.resolve(path).id, id, "{path}");
        }
        // Content facets stay Rust-only.
        let pb = reg.enrich_with_content_facets(reg.resolve("a.cpp"), &[("a.cpp", "unsafe {")]);
        assert!(pb.active_facets.is_empty());
    }

    // trace:STORY-3120 | ai:claude
    #[test]
    fn s6_junit_and_testbench_companions_share_a_stem() {
        let pairs = [
            (
                "src/main/java/com/acme/Parser.java",
                "src/test/java/com/acme/ParserTest.java",
            ),
            (
                "src/main/kotlin/Parser.kt",
                "src/test/kotlin/ParserTests.kt",
            ),
            ("rtl/axi_fifo.sv", "rtl/axi_fifo_tb.sv"),
            ("rtl/uart_rx.vhd", "rtl/tb_uart_rx.vhd"),
        ];
        for (src, test) in pairs {
            assert_eq!(
                canonical_module_stem(src),
                canonical_module_stem(test),
                "{test}"
            );
        }
        assert_eq!(canonical_module_stem("src/contest.rs"), "src/contest");
        assert_eq!(canonical_module_stem("src/Latest.java"), "src/latest");
        assert_eq!(canonical_module_stem("src/Test.java"), "src/test");
        assert_eq!(canonical_module_stem("src/Foo_Test.java"), "src/foo");
        assert_eq!(canonical_module_stem("rtl/tb_.sv"), "rtl/tb_");
        // Other directories never pair across the mirror.
        assert_ne!(
            canonical_module_stem("a/src/main/java/Parser.java"),
            canonical_module_stem("b/src/test/java/ParserTest.java")
        );
        let file = |path: &str| ReviewableFile {
            path: path.into(),
            old_path: None,
            kind: FileChangeKind::Modified,
            additions: 10,
            deletions: 0,
            estimated_tokens: 200,
            playbook_id: "hdl_rtl".into(),
            has_trace_comment: false,
        };
        let batches =
            bundle_review_batches(&[file("rtl/fifo.sv"), file("rtl/fifo_tb.sv")], 10_000, 8)
                .unwrap();
        assert_eq!(batches.len(), 1);
        assert!(
            batches[0].label.starts_with("module:"),
            "{:?}",
            batches[0].label
        );
    }
}
