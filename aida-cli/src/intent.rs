//! `aida intent <spec>` — the AI-generated, cached, drift-stamped plain-terms
//! comprehension of WHY a spec exists.
//!
//! Distinct from `aida why` (a deterministic Rust classifier over store
//! signals): intent is an LLM SYNTHESIS task — it reads the spec + its
//! immediate graph neighborhood and distils reason-for-being into prose.
//! It is therefore GENERATED (an AI pass), CACHED on the spec
//! ([`aida_core::SpecIntent`]), and DRIFT-STAMPED via a `source_hash` over the
//! neighborhood inputs so it regenerates when the spec or its neighbors change.
//!
//! This module holds the pure, unit-tested parts — the neighborhood-hash
//! input assembly, the stale computation, and the `--json` shape. The
//! integration boundary (store load/save + the headless `claude -p` spawn that
//! runs the `/aida-intent` skill) lives in `main.rs`, mirroring how `intake.rs`
//! pairs with `handle_intake_command`.
//!
//! trace:STORY-631 | ai:claude

use aida_core::Requirement;
use serde::{Deserialize, Serialize};

/// A single immediate-neighbor fact contributing to the drift hash.
///
/// We hash the neighbor's *identity + coarse state* (id, title, status), NOT
/// its full body: the comprehension is about how this spec relates to its
/// neighbors, so a neighbor's title or status flipping is a drift signal, but
/// an unrelated description tweak deep inside a neighbor is not. Keeps the hash
/// stable against churn that does not change the spec's reason-for-being.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeighborFact {
    pub id: String,
    pub title: String,
    pub status: String,
}

/// The full set of inputs that, when changed, mean the cached comprehension is
/// stale and should be regenerated.
///
/// CHOSEN INPUT SET (the open fork the spec left to the implementer): the
/// spec's OWN title + description + status, plus, for each immediate neighbor
/// (parents, children, blockers/blocked, references, decisions), the neighbor's
/// id + title + status, plus the spec's key-comment COUNT. Rationale:
/// - Own title/description/status: the spec's intent is primarily its own text;
///   a status flip changes "why it's still open" framing.
/// - Neighbor id/title/status (not body): the *relationship web* is what makes
///   intent hard for a human and tractable for an LLM; a neighbor appearing,
///   disappearing, being retitled, or changing state is a real drift signal,
///   but we deliberately do NOT recurse into neighbor descriptions (too noisy,
///   and second-order to this spec's reason-for-being).
/// - Comment count (not bodies): new discussion is a cheap, monotonic "the
///   conversation moved" signal without hashing volatile prose.
/// Neighbors are sorted by id so the hash is order-independent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeighborhoodInputs {
    pub spec_id: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub neighbors: Vec<NeighborFact>,
    pub comment_count: usize,
}

impl NeighborhoodInputs {
    /// Canonical, order-independent serialization of the inputs. Neighbors are
    /// sorted by id; the format is a stable line-oriented digest input so the
    /// same neighborhood always produces the same string (and thus hash).
    pub fn canonical(&self) -> String {
        let mut neighbors = self.neighbors.clone();
        neighbors.sort_by(|a, b| a.id.cmp(&b.id));
        let mut s = String::new();
        s.push_str("spec:");
        s.push_str(&self.spec_id);
        s.push('\n');
        s.push_str("title:");
        s.push_str(&self.title);
        s.push('\n');
        s.push_str("status:");
        s.push_str(&self.status);
        s.push('\n');
        s.push_str("description:");
        s.push_str(&self.description);
        s.push('\n');
        s.push_str("comments:");
        s.push_str(&self.comment_count.to_string());
        s.push('\n');
        for n in &neighbors {
            s.push_str("neighbor:");
            s.push_str(&n.id);
            s.push('|');
            s.push_str(&n.title);
            s.push('|');
            s.push_str(&n.status);
            s.push('\n');
        }
        s
    }

    /// The drift hash over the canonical inputs. Hex-encoded FNV-1a-64 — a
    /// fast, dependency-free, deterministic digest (the same family AIDA uses
    /// for the memory-pack `scaffoldChecksum`). The exact algorithm does not
    /// matter as long as it is stable across runs and platforms; collisions are
    /// irrelevant here because a miss only means "regenerate", never a wrong
    /// answer.
    pub fn source_hash(&self) -> String {
        fnv1a_hex(self.canonical().as_bytes())
    }
}

/// FNV-1a 64-bit, hex-encoded. Stable across platforms (no float, no endian
/// dependence). trace:STORY-631 | ai:claude
pub fn fnv1a_hex(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// A cached comprehension is STALE when the stored `source_hash` differs from a
/// freshly-computed neighborhood hash. Computed at READ time, never stored.
pub fn is_stale(stored_source_hash: &str, fresh_source_hash: &str) -> bool {
    stored_source_hash != fresh_source_hash
}

/// The pickup-brief decision for a spec's cached comprehension (TASK-838).
///
/// `/aida-pickup` and the `aida queue work` brief lead the implementer context
/// with the spec's cached `llm`-register comprehension when one exists AND is
/// fresh. This enum is the pure, testable decision — given the stored intent
/// (an `Option`) and the freshly-computed neighborhood hash, it says what the
/// brief should render. Pickup must stay fast and non-AI-blocking, so a missing
/// or stale comprehension is a ONE-LINE note, never an inline generation.
/// trace:TASK-838 | ai:claude
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickupIntent {
    /// Fresh comprehension — render the `llm` register at the top of the brief,
    /// labeled AI-generated. Carries the prose, the model, and the generated-at
    /// stamp so the caller can label provenance.
    Render {
        llm: String,
        model: String,
        generated_at: String,
    },
    /// Absent or stale — emit a one-line note nudging `aida intent <spec>` and
    /// continue. `stale` distinguishes the two for the wording.
    Note { stale: bool },
}

/// Decide what the pickup brief should render for a spec's cached comprehension.
///
/// FEATURE-DETECT the `Option`: a spec with no `intent` (STORY-631 not run for
/// it, or not shipped at all) yields `Note { stale: false }` — an absent note,
/// no behavior change beyond the one line. A present-but-stale comprehension
/// (neighborhood hash drifted) yields `Note { stale: true }`. Only a present,
/// fresh comprehension yields `Render`. Reuses [`is_stale`] for the drift call —
/// the same comparator `aida intent` uses — so the two surfaces never diverge.
/// trace:TASK-838 | ai:claude
pub fn decide_pickup_intent(
    intent: Option<&aida_core::SpecIntent>,
    fresh_source_hash: &str,
) -> PickupIntent {
    match intent {
        None => PickupIntent::Note { stale: false },
        Some(i) => {
            if is_stale(&i.source_hash, fresh_source_hash) {
                PickupIntent::Note { stale: true }
            } else {
                PickupIntent::Render {
                    llm: i.llm.clone(),
                    model: i.model.clone(),
                    generated_at: i.generated_at.clone(),
                }
            }
        }
    }
}

/// The one-line note text for the absent / stale cases. Names the spec and the
/// remedy (`aida intent <spec>`) without leaking the SPEC-ID into a banner — the
/// id is the developer breadcrumb the implementer already holds at pickup.
/// trace:TASK-838 | ai:claude
pub fn pickup_intent_note(disp: &str, stale: bool) -> String {
    if stale {
        format!("stale intent comprehension — run `aida intent {disp}` to refresh")
    } else {
        format!("no intent comprehension — run `aida intent {disp}` to generate")
    }
}

/// The `--json` payload for `aida intent`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntentJson {
    pub spec: String,
    pub audience: String,
    pub comprehension: String,
    pub generated_at: String,
    pub model: String,
    pub stale: bool,
}

/// Build the `--json` payload from a stored [`aida_core::SpecIntent`], the
/// requested audience register, the display id, and the freshly-computed
/// staleness.
pub fn intent_json(
    spec: &str,
    audience: &str,
    intent: &aida_core::SpecIntent,
    stale: bool,
) -> IntentJson {
    let comprehension = match audience {
        "llm" => intent.llm.clone(),
        _ => intent.layman.clone(),
    };
    IntentJson {
        spec: spec.to_string(),
        audience: audience.to_string(),
        comprehension,
        generated_at: intent.generated_at.clone(),
        model: intent.model.clone(),
        stale,
    }
}

/// The JSON sidecar the `/aida-intent` skill writes — the two registers plus
/// the model that produced them. The launcher reads this back and folds it into
/// the stored [`aida_core::SpecIntent`] (adding generated_at + source_hash).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntentSidecar {
    pub layman: String,
    pub llm: String,
    /// The model that produced the comprehension. The skill fills this with its
    /// own model id; absent ⇒ the launcher falls back to a generic label.
    #[serde(default)]
    pub model: String,
}

/// Parse the skill's sidecar JSON. Tolerant of trailing prose/log noise around
/// the JSON object the way `claude -p` sometimes emits, by extracting the first
/// balanced `{...}` span if a direct parse fails.
pub fn parse_intent_sidecar(raw: &str) -> anyhow::Result<IntentSidecar> {
    if let Ok(s) = serde_json::from_str::<IntentSidecar>(raw.trim()) {
        return Ok(s);
    }
    if let (Some(start), Some(end)) = (raw.find('{'), raw.rfind('}')) {
        if end > start {
            if let Ok(s) = serde_json::from_str::<IntentSidecar>(&raw[start..=end]) {
                return Ok(s);
            }
        }
    }
    anyhow::bail!("could not parse intent sidecar JSON from the skill output")
}

/// True when this spec carries enough that a "key comment" likely matters — we
/// count ALL comments as the cheap drift signal (see [`NeighborhoodInputs`]).
pub fn key_comment_count(req: &Requirement) -> usize {
    req.comments.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_inputs() -> NeighborhoodInputs {
        NeighborhoodInputs {
            spec_id: "STORY-631".to_string(),
            title: "aida intent".to_string(),
            description: "AI WHY-comprehension".to_string(),
            status: "approved".to_string(),
            neighbors: vec![
                NeighborFact {
                    id: "STORY-630".to_string(),
                    title: "schema explain".to_string(),
                    status: "draft".to_string(),
                },
                NeighborFact {
                    id: "ADR-5".to_string(),
                    title: "living docs".to_string(),
                    status: "accepted".to_string(),
                },
            ],
            comment_count: 2,
        }
    }

    #[test]
    fn hash_is_deterministic() {
        let a = sample_inputs();
        let b = sample_inputs();
        assert_eq!(a.source_hash(), b.source_hash());
    }

    #[test]
    fn hash_is_order_independent_over_neighbors() {
        let a = sample_inputs();
        let mut b = sample_inputs();
        b.neighbors.reverse();
        assert_eq!(
            a.source_hash(),
            b.source_hash(),
            "neighbor ordering must not change the hash"
        );
    }

    #[test]
    fn hash_changes_when_neighbor_status_flips() {
        let a = sample_inputs();
        let mut b = sample_inputs();
        b.neighbors[0].status = "completed".to_string();
        assert_ne!(a.source_hash(), b.source_hash());
    }

    #[test]
    fn hash_changes_when_own_status_flips() {
        let a = sample_inputs();
        let mut b = sample_inputs();
        b.status = "completed".to_string();
        assert_ne!(a.source_hash(), b.source_hash());
    }

    #[test]
    fn hash_changes_when_a_neighbor_appears() {
        let a = sample_inputs();
        let mut b = sample_inputs();
        b.neighbors.push(NeighborFact {
            id: "TASK-838".to_string(),
            title: "pickup wiring".to_string(),
            status: "draft".to_string(),
        });
        assert_ne!(a.source_hash(), b.source_hash());
    }

    #[test]
    fn hash_changes_when_comment_count_changes() {
        let a = sample_inputs();
        let mut b = sample_inputs();
        b.comment_count = 3;
        assert_ne!(a.source_hash(), b.source_hash());
    }

    #[test]
    fn stale_when_hashes_differ() {
        assert!(is_stale("aaaa", "bbbb"));
        assert!(!is_stale("aaaa", "aaaa"));
    }

    #[test]
    fn spec_intent_round_trips_serde_and_skips_when_none() {
        // A spec with no intent must serialize with NO `intent:` key.
        let req = Requirement::new("t".to_string(), "d".to_string());
        let yaml = serde_yaml::to_string(&req).unwrap();
        assert!(
            !yaml.contains("intent:"),
            "absent intent must be skip_serializing_if none"
        );

        // A spec WITH intent must round-trip every field.
        let mut req2 = Requirement::new("t".to_string(), "d".to_string());
        req2.intent = Some(aida_core::SpecIntent {
            layman: "plain why".to_string(),
            llm: "dense why".to_string(),
            generated_at: "2026-06-15T00:00:00Z".to_string(),
            source_hash: "deadbeef".to_string(),
            model: "claude-opus".to_string(),
        });
        let yaml2 = serde_yaml::to_string(&req2).unwrap();
        assert!(yaml2.contains("intent:"));
        let back: Requirement = serde_yaml::from_str(&yaml2).unwrap();
        assert_eq!(back.intent, req2.intent);
    }

    fn sample_intent(source_hash: &str) -> aida_core::SpecIntent {
        aida_core::SpecIntent {
            layman: "plain why".to_string(),
            llm: "dense why".to_string(),
            generated_at: "2026-06-15T00:00:00Z".to_string(),
            source_hash: source_hash.to_string(),
            model: "claude-opus".to_string(),
        }
    }

    // TASK-838: the pickup-brief decision — present+fresh renders the llm
    // register at top; absent emits a note; stale emits a note. Reuses the
    // is_stale comparator (via decide_pickup_intent) so the brief and
    // `aida intent` agree on drift. trace:TASK-838
    #[test]
    fn pickup_renders_when_intent_present_and_fresh() {
        let intent = sample_intent("abc123");
        let got = decide_pickup_intent(Some(&intent), "abc123");
        assert_eq!(
            got,
            PickupIntent::Render {
                llm: "dense why".to_string(),
                model: "claude-opus".to_string(),
                generated_at: "2026-06-15T00:00:00Z".to_string(),
            },
            "a fresh comprehension must render the llm register"
        );
    }

    #[test]
    fn pickup_notes_when_intent_absent() {
        let got = decide_pickup_intent(None, "abc123");
        assert_eq!(
            got,
            PickupIntent::Note { stale: false },
            "no cached comprehension → absent note, no behavior change"
        );
        assert!(pickup_intent_note("TASK-838", false).contains("no intent"));
        assert!(pickup_intent_note("TASK-838", false).contains("aida intent TASK-838"));
    }

    #[test]
    fn pickup_notes_when_intent_stale() {
        // Stored hash differs from the freshly-computed neighborhood hash →
        // is_stale(...) is true → a stale note, never an inline regeneration.
        let intent = sample_intent("OLD-hash");
        let got = decide_pickup_intent(Some(&intent), "NEW-hash");
        assert_eq!(
            got,
            PickupIntent::Note { stale: true },
            "a drifted comprehension must note stale, not render"
        );
        assert!(pickup_intent_note("TASK-838", true).contains("stale intent"));
        assert!(pickup_intent_note("TASK-838", true).contains("aida intent TASK-838"));
    }

    #[test]
    fn json_shape_selects_register_and_carries_fields() {
        let intent = aida_core::SpecIntent {
            layman: "plain".to_string(),
            llm: "dense".to_string(),
            generated_at: "2026-06-15T00:00:00Z".to_string(),
            source_hash: "abc".to_string(),
            model: "claude-opus".to_string(),
        };
        let layman = intent_json("STORY-631", "layman", &intent, false);
        assert_eq!(layman.comprehension, "plain");
        assert_eq!(layman.spec, "STORY-631");
        assert_eq!(layman.audience, "layman");
        assert_eq!(layman.model, "claude-opus");
        assert!(!layman.stale);

        let llm = intent_json("STORY-631", "llm", &intent, true);
        assert_eq!(llm.comprehension, "dense");
        assert!(llm.stale);

        // Serializes to the documented key set.
        let v: serde_json::Value = serde_json::to_value(&layman).unwrap();
        for key in [
            "spec",
            "audience",
            "comprehension",
            "generated_at",
            "model",
            "stale",
        ] {
            assert!(v.get(key).is_some(), "missing key {key}");
        }
    }

    #[test]
    fn sidecar_parses_clean_and_noisy() {
        let clean = r#"{"layman":"why prose","llm":"dense","model":"claude-opus"}"#;
        let s = parse_intent_sidecar(clean).unwrap();
        assert_eq!(s.layman, "why prose");
        assert_eq!(s.model, "claude-opus");

        let noisy = "Here is the result:\n{\"layman\":\"a\",\"llm\":\"b\",\"model\":\"m\"}\nDone.";
        let s2 = parse_intent_sidecar(noisy).unwrap();
        assert_eq!(s2.llm, "b");

        // Model is optional.
        let no_model = r#"{"layman":"a","llm":"b"}"#;
        let s3 = parse_intent_sidecar(no_model).unwrap();
        assert_eq!(s3.model, "");

        assert!(parse_intent_sidecar("not json at all").is_err());
    }
}
