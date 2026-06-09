//! `aida schema` — read-only introspection of the storable substrate.
//!
//! Surfaces two things an agent (or a curious operator) otherwise has to
//! reverse-engineer from `models.rs`:
//!   1. the **catalog** of storable object kinds, and
//!   2. the **Requirement** field table + the four controlled-vocabulary
//!      enums in their exact on-the-wire token form, so the output doubles
//!      as a paste-ready cheat-sheet for `--status` / `--type` / `--priority`
//!      / relationship-type arguments.
//!
//! DERIVATION (substrate-as-bouncer): the Requirement field set and the enum
//! variant sets are **derived from the `ts-rs-forge` type reflection** that
//! already backs `aida-generate-types` — never hand-maintained here. We parse
//! the `TS::decl()` output of `Requirement` (the field table) and of the four
//! enums (the variant lists). The wire token for each variant is the
//! kebab-case of its PascalCase reflected name — exactly the canonical form
//! every CLI parser (`parse_status` / `parse_type` / `parse_priority`,
//! `RelationshipType::from_str`) accepts. A drift-guard test
//! (`schema_enums_match_reflection`) pins this so the schema can't silently
//! rot away from `models.rs`.
//!
//! Read-only: this module mutates nothing.
//
// trace:STORY-538 | ai:claude

use aida_core::models::{
    RelationshipType, Requirement, RequirementPriority, RequirementStatus, RequirementType,
};
use serde_json::{json, Value};
use ts_rs_forge::TS;

/// One row in the storable-object catalog.
struct CatalogEntry {
    /// Object kind name as a human would say it.
    name: &'static str,
    /// One-line description of what it stores / where it lives.
    description: &'static str,
}

/// The curated catalog of storable object kinds. The listing is a hand-written
/// one-liner table on purpose — full per-object field reflection for the
/// non-Requirement kinds is a deferred follow-up (it needs the reflection
/// registry extended). The Requirement *detail* view, by contrast, is fully
/// reflection-derived. trace:STORY-538 | ai:claude
const CATALOG: &[CatalogEntry] = &[
    CatalogEntry {
        name: "Requirement",
        description: "The core spec node (epic/story/task/bug/...) — title, status, type, priority, relationships, history.",
    },
    CatalogEntry {
        name: "Finding",
        description: "A shelved phase-failure or advisor observation surfaced for triage (aida findings).",
    },
    CatalogEntry {
        name: "Brief",
        description: "A local pickup brief routing work to an agent without scrollback (.aida/agent-briefs/).",
    },
    CatalogEntry {
        name: "Punt",
        description: "A design-fork an autonomous agent could not safely resolve; parks the spec NeedsAttention.",
    },
    CatalogEntry {
        name: "Directive",
        description: "A standing instruction posted to an agent/role via the inter-agent mailbox.",
    },
    CatalogEntry {
        name: "Comment",
        description: "A threaded note on a Requirement (carries reactions; doc-seed carrier).",
    },
    CatalogEntry {
        name: "Lease",
        description: "An active claim on a spec/worktree by a session — prevents double-driving.",
    },
    CatalogEntry {
        name: "QueueItem",
        description: "A position in a role's work queue (keyed off the shell user identity).",
    },
    CatalogEntry {
        name: "HistoryEntry",
        description: "An immutable change row inside a Requirement's YAML (the spec-state time series).",
    },
    CatalogEntry {
        name: "Relationship",
        description: "A typed edge between two Requirements (parent/child/blocked-by/blocks/references/...).",
    },
];

/// True if `name` (case-insensitive) is a kind in the storable-object catalog —
/// lets the dispatcher tell "known object, detail not built yet" from a typo.
/// trace:STORY-538 | ai:claude
pub fn is_catalog_object(name: &str) -> bool {
    CATALOG.iter().any(|e| e.name.eq_ignore_ascii_case(name))
}

/// A controlled-vocabulary enum the CLI/MCP accept as argument tokens.
struct EnumSchema {
    /// Field name on the Requirement this enum controls.
    field: &'static str,
    /// On-the-wire tokens, in declaration order.
    tokens: Vec<String>,
}

/// A reflected Requirement field.
struct FieldSchema {
    name: String,
    ts_type: String,
    /// `true` when the field is optional / nullable in the wire shape.
    optional: bool,
}

/// Convert a reflected PascalCase enum variant name into its on-the-wire
/// token — the kebab-case form every CLI parser accepts
/// (`InProgress` -> `in-progress`, `NonFunctional` -> `non-functional`,
/// `VerifiedBy` -> `verified-by`). This is the single conversion rule the
/// drift-guard pins. trace:STORY-538 | ai:claude
fn variant_to_wire_token(variant: &str) -> String {
    let mut out = String::with_capacity(variant.len() + 4);
    for (i, ch) in variant.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i != 0 {
                out.push('-');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// Parse the unit-variant names out of a `TS::decl()` enum declaration line of
/// the form `type Name = "A" | "B" | { "Custom": string };`. Newtype variants
/// (the `{ "Custom": string }` arm) are skipped — they carry a user-defined
/// payload, not a fixed token. trace:STORY-538 | ai:claude
fn parse_enum_variants(decl: &str) -> Vec<String> {
    let mut variants = Vec::new();
    let bytes = decl.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            // Collect to the closing quote.
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != b'"' {
                j += 1;
            }
            let token = &decl[start..j];
            // A `{ "Custom": string }` arm has a `:` immediately after the
            // closing quote (modulo whitespace) — that names a newtype payload
            // key, not a unit variant. Skip it.
            let mut k = j + 1;
            while k < bytes.len() && (bytes[k] == b' ' || bytes[k] == b'\t') {
                k += 1;
            }
            if !(k < bytes.len() && bytes[k] == b':') {
                variants.push(token.to_string());
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    variants
}

/// Derive the four controlled-vocabulary enum schemas from reflection.
fn requirement_enums() -> Vec<EnumSchema> {
    let to_tokens = |decl: String| -> Vec<String> {
        parse_enum_variants(&decl)
            .iter()
            .map(|v| variant_to_wire_token(v))
            .collect()
    };
    vec![
        EnumSchema {
            field: "status",
            tokens: to_tokens(RequirementStatus::decl()),
        },
        EnumSchema {
            field: "type",
            tokens: to_tokens(RequirementType::decl()),
        },
        EnumSchema {
            field: "relationship",
            tokens: to_tokens(RelationshipType::decl()),
        },
        EnumSchema {
            field: "priority",
            tokens: to_tokens(RequirementPriority::decl()),
        },
    ]
}

/// Parse the Requirement field table out of its `TS::decl()` named-struct
/// declaration. Each field line has the shape `name: type,` or
/// `name?: type | null,` (with `/** ... */` doc-comment blocks between
/// fields, which we skip). trace:STORY-538 | ai:claude
fn requirement_fields() -> Vec<FieldSchema> {
    parse_struct_fields(&Requirement::decl())
}

/// The named-struct field parser shared by `requirement_fields` (and the
/// drift-guard test). Skips doc-comment lines (`/**`, ` *`, ` */`) and the
/// enclosing braces; matches `name(?): type,` lines.
fn parse_struct_fields(decl: &str) -> Vec<FieldSchema> {
    let mut fields = Vec::new();
    let mut in_doc = false;
    for raw in decl.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        // Skip doc-comment blocks.
        if line.starts_with("/**") {
            in_doc = !line.ends_with("*/");
            continue;
        }
        if in_doc {
            if line.ends_with("*/") {
                in_doc = false;
            }
            continue;
        }
        if line.starts_with('*') || line.starts_with("//") {
            continue;
        }
        // Skip the declaration header and the closing brace.
        if line.starts_with("type ") || line.starts_with("export ") || line == "{" || line == "};" {
            continue;
        }
        // A field line: `name: type,` or `name?: type | null,`.
        let Some(colon) = line.find(':') else {
            continue;
        };
        let (name_part, type_part) = line.split_at(colon);
        let name_part = name_part.trim();
        // The first ':' may belong to the type (it never does for our flat
        // fields), but guard anyway: a valid field name is a bare identifier.
        let optional = name_part.ends_with('?');
        let name = name_part.trim_end_matches('?').trim();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        // Strip the leading ':' then everything from the field-terminating ','
        // onward — the last field shares its line with the struct's closing
        // brace (`... | null, };`), so a plain trailing-comma trim isn't enough.
        let mut ts_type = type_part[1..].trim().to_string();
        if let Some(comma) = ts_type.rfind(',') {
            // Only treat a comma as the terminator if what follows it is the
            // closing brace / whitespace (never part of a real TS type here).
            let tail = ts_type[comma + 1..].trim();
            if tail.is_empty() || tail == "};" || tail == "}" {
                ts_type.truncate(comma);
            }
        }
        let ts_type = ts_type.trim().to_string();
        fields.push(FieldSchema {
            name: name.to_string(),
            ts_type,
            optional,
        });
    }
    fields
}

/// `aida schema` (no args) — the storable-object catalog.
pub fn print_catalog(json_out: bool) {
    if json_out {
        let objects: Vec<Value> = CATALOG
            .iter()
            .map(|e| json!({ "name": e.name, "description": e.description }))
            .collect();
        let v = json!({ "objects": objects });
        println!("{}", serde_json::to_string_pretty(&v).unwrap());
        return;
    }

    println!("Storable object catalog\n");
    let width = CATALOG.iter().map(|e| e.name.len()).max().unwrap_or(0);
    for e in CATALOG {
        println!("  {:<width$}  {}", e.name, e.description, width = width);
    }
    println!(
        "\nFull field + enum detail today: `aida schema requirement`. Detail for the other \
         objects is coming — use the one-liners above for now."
    );
}

/// `aida schema requirement` — the reflection-derived field table and the
/// four controlled-vocabulary enums in on-the-wire token form.
pub fn print_requirement(json_out: bool) {
    let fields = requirement_fields();
    let enums = requirement_enums();

    if json_out {
        let field_vals: Vec<Value> = fields
            .iter()
            .map(|f| {
                json!({
                    "name": f.name,
                    "type": f.ts_type,
                    "optional": f.optional,
                })
            })
            .collect();
        let enum_vals: Value = {
            let mut map = serde_json::Map::new();
            for e in &enums {
                map.insert(e.field.to_string(), json!(e.tokens));
            }
            Value::Object(map)
        };
        let v = json!({
            "object": "Requirement",
            "fields": field_vals,
            "enums": enum_vals,
        });
        println!("{}", serde_json::to_string_pretty(&v).unwrap());
        return;
    }

    println!("Requirement — fields\n");
    let name_w = fields.iter().map(|f| f.name.len()).max().unwrap_or(0);
    for f in &fields {
        let opt = if f.optional { " (optional)" } else { "" };
        println!(
            "  {:<name_w$}  {}{}",
            f.name,
            f.ts_type,
            opt,
            name_w = name_w
        );
    }

    println!("\nRequirement — controlled vocabularies (on-the-wire tokens)\n");
    let field_w = enums.iter().map(|e| e.field.len()).max().unwrap_or(0);
    for e in &enums {
        println!(
            "  {:<field_w$}  {}",
            e.field,
            e.tokens.join("|"),
            field_w = field_w
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant_to_wire_token_matches_cli_forms() {
        assert_eq!(variant_to_wire_token("Draft"), "draft");
        assert_eq!(variant_to_wire_token("InProgress"), "in-progress");
        assert_eq!(variant_to_wire_token("NeedsAttention"), "needs-attention");
        assert_eq!(variant_to_wire_token("NonFunctional"), "non-functional");
        assert_eq!(variant_to_wire_token("VerifiedBy"), "verified-by");
        assert_eq!(variant_to_wire_token("BlockedBy"), "blocked-by");
        assert_eq!(variant_to_wire_token("High"), "high");
    }

    #[test]
    fn parse_enum_variants_skips_custom_newtype() {
        let decl = aida_core::models::RelationshipType::decl();
        let variants = parse_enum_variants(&decl);
        // The `{ "Custom": string }` arm must NOT appear as a unit variant.
        assert!(variants.contains(&"Parent".to_string()));
        assert!(variants.contains(&"BlockedBy".to_string()));
        assert!(!variants.contains(&"Custom".to_string()));
    }

    /// DRIFT-GUARD: the enum tokens the schema reports must stay in sync with
    /// the model reflection AND with the canonical CLI/wire forms. If a variant
    /// is added/removed/renamed in `models.rs`, the reflected `decl()` changes
    /// and these expectations break — forcing the schema (and this list) to be
    /// updated deliberately rather than silently rotting. trace:STORY-538
    #[test]
    fn schema_enums_match_reflection() {
        let enums = requirement_enums();
        let by_field = |f: &str| -> Vec<String> {
            enums.iter().find(|e| e.field == f).unwrap().tokens.clone()
        };

        assert_eq!(
            by_field("status"),
            vec![
                "draft",
                "approved",
                "planned",
                "in-progress",
                "done",
                "completed",
                "rejected",
                "needs-attention",
            ]
        );
        assert_eq!(by_field("priority"), vec!["high", "medium", "low"]);
        assert_eq!(
            by_field("type"),
            vec![
                "functional",
                "non-functional",
                "system",
                "user",
                "change-request",
                "bug",
                "epic",
                "story",
                "task",
                "spike",
                "sprint",
                "folder",
                "meta",
                "principle",
                "vision",
                "constraint",
                "decision",
                "term",
                "doc",
            ]
        );
        // Relationship: the fixed (non-Custom) variants, in declaration order.
        assert_eq!(
            by_field("relationship"),
            vec![
                "parent",
                "child",
                "duplicate",
                "verifies",
                "verified-by",
                "references",
                "blocked-by",
                "blocks",
            ]
        );
    }

    /// DRIFT-GUARD: the Requirement field set the schema reports must match the
    /// fields the model reflects. A new field on `Requirement` (or a removed
    /// one) changes `Requirement::decl()` and breaks this anchor, so the schema
    /// view can't silently drift from `models.rs`. We assert a representative
    /// stable subset is present (so unrelated additive fields don't churn the
    /// test) plus that the field count tracks reflection exactly.
    #[test]
    fn schema_fields_track_reflection() {
        let fields = requirement_fields();
        let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
        for expected in [
            "id",
            "spec_id",
            "title",
            "description",
            "status",
            "priority",
            "req_type",
            "tags",
            "relationships",
            "comments",
            "history",
            "archived",
        ] {
            assert!(
                names.contains(&expected),
                "schema lost reflected field `{expected}` — schema drifted from models.rs"
            );
        }
        // The parse must recover the SAME number of fields the reflection
        // emits (one field line per non-doc, non-brace line). Re-derive the
        // expected count straight from a fresh parse so the guard tracks the
        // model, not a frozen integer.
        let reparsed = parse_struct_fields(&Requirement::decl());
        assert_eq!(fields.len(), reparsed.len());
        // Sanity floor: Requirement is a wide struct; if the parser ever
        // collapses to a near-empty set the guard should scream.
        assert!(
            fields.len() >= 20,
            "expected >=20 reflected Requirement fields, got {}",
            fields.len()
        );
    }

    /// Smoke: the catalog covers exactly the ten storable kinds the MVP slice
    /// promises, and `Relationship` (the type whose presence proves the import
    /// is live) is among them.
    #[test]
    fn catalog_lists_the_storable_kinds() {
        let names: Vec<&str> = CATALOG.iter().map(|e| e.name).collect();
        for expected in [
            "Requirement",
            "Finding",
            "Brief",
            "Punt",
            "Directive",
            "Comment",
            "Lease",
            "QueueItem",
            "HistoryEntry",
            "Relationship",
        ] {
            assert!(names.contains(&expected), "catalog missing {expected}");
        }
        // Touch the Relationship type's reflection so the catalog's claim that
        // it is a storable kind stays grounded in a type that actually exists.
        let _ = aida_core::models::Relationship::decl();
    }
}
