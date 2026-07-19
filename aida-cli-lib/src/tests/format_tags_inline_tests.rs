use super::format_tags_inline;

fn s(items: &[&str]) -> Vec<String> {
    items.iter().map(|x| x.to_string()).collect()
}

#[test]
fn empty_tags_render_empty() {
    assert_eq!(format_tags_inline(&[], 3), "");
}

#[test]
fn fewer_than_cap_renders_all_with_no_suffix() {
    assert_eq!(format_tags_inline(&s(&["a", "b"]), 3), "a, b");
}

#[test]
fn exactly_at_cap_renders_all_with_no_suffix() {
    assert_eq!(format_tags_inline(&s(&["a", "b", "c"]), 3), "a, b, c");
}

#[test]
fn over_cap_truncates_with_more_suffix() {
    assert_eq!(
        format_tags_inline(&s(&["a", "b", "c", "d", "e"]), 3),
        "a, b, c +2 more"
    );
}

#[test]
fn zero_cap_renders_empty() {
    assert_eq!(format_tags_inline(&s(&["a", "b"]), 0), "");
}
