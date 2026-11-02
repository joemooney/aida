use super::*;

const PRODUCT: &str = "# Product glossary";
const MACHINERY: &str = "# Machinery glossary";
const LIFECYCLE: &str = "# Lifecycle vocabulary";

/// TASK-589 / STORY-2219: with no flags, every glossary section renders, in
/// the order product, machinery, lifecycle.
/// trace:STORY-2219 | ai:claude
#[test]
fn default_renders_all_glossary_sections_product_first() {
    let out = render_discipline_glossary(false, false, false).unwrap();
    let p = out.find(PRODUCT).expect("product section present");
    let m = out.find(MACHINERY).expect("machinery section present");
    let l = out.find(LIFECYCLE).expect("lifecycle section present");
    assert!(p < m, "product glossary renders before machinery glossary");
    assert!(
        m < l,
        "machinery glossary renders before lifecycle vocabulary"
    );
}

/// Each filter flag isolates its own section.
#[test]
fn filter_flags_isolate_their_section() {
    let product = render_discipline_glossary(true, false, false).unwrap();
    assert!(product.contains(PRODUCT));
    assert!(
        !product.contains(MACHINERY) && !product.contains(LIFECYCLE),
        "--product omits the other sections"
    );

    let machinery = render_discipline_glossary(false, true, false).unwrap();
    assert!(machinery.contains(MACHINERY));
    assert!(
        !machinery.contains(LIFECYCLE) && !machinery.contains(PRODUCT),
        "--machinery omits the other sections"
    );

    let lifecycle = render_discipline_glossary(false, false, true).unwrap();
    assert!(lifecycle.contains(LIFECYCLE));
    assert!(
        !lifecycle.contains(MACHINERY) && !lifecycle.contains(PRODUCT),
        "--lifecycle omits the other sections"
    );
}

/// Two flags print exactly those two sections; all three print everything.
#[test]
fn combined_flags_render_only_the_named_sections() {
    let out = render_discipline_glossary(false, true, true).unwrap();
    assert!(out.contains(MACHINERY) && out.contains(LIFECYCLE));
    assert!(!out.contains(PRODUCT));
    assert_eq!(
        render_discipline_glossary(true, true, true).unwrap(),
        render_discipline_glossary(false, false, false).unwrap()
    );
}

/// The product glossary carries the first batch of end-user terms the website
/// glossary anchors on.
#[test]
fn product_glossary_defines_the_first_batch_of_terms() {
    let out = render_discipline_glossary(true, false, false).unwrap();
    for term in [
        "Merge hold",
        "Review story",
        "Rework round",
        "Brief",
        "Mailbox",
        "Handoff",
        "Commit trailer",
        "Grant",
        "Child grant",
        "Roster",
        "Seat",
        "Role",
        "Mandate",
        "Deputy",
        "Operator-only",
        "Supervised spec",
        "Hub",
        "Ref",
        "Pool",
        "Block",
        "Root",
        "Agreed ID",
        "Local ID",
    ] {
        assert!(
            out.contains(&format!("\n### {term}\n")),
            "product glossary defines `{term}`"
        );
    }
}
