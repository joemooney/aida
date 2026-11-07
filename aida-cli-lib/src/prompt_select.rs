// trace:TASK-3325 | ai:codex
//! Keep fuzzy search in CLI pickers without inquire's unmaintained matcher.

use nucleo_matcher::pattern::{Atom, AtomKind, CaseMatching, Normalization};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use std::cell::RefCell;
use std::fmt::Display;

thread_local! {
    static MATCHER: RefCell<Matcher> = RefCell::new(Matcher::new(Config::DEFAULT));
}

fn score(input: &str, label: &str) -> Option<i64> {
    if input.is_empty() {
        return Some(0);
    }
    // Match literal input, not nucleo's optional query operators or word syntax.
    let atom = Atom::new(
        input,
        CaseMatching::Ignore,
        Normalization::Never,
        AtomKind::Fuzzy,
        false,
    );
    let mut buffer = Vec::new();
    MATCHER.with(|matcher| {
        atom.score(Utf32Str::new(label, &mut buffer), &mut matcher.borrow_mut())
            .map(i64::from)
    })
}

pub(crate) fn select<'a, T: Display>(message: &'a str, options: Vec<T>) -> inquire::Select<'a, T> {
    inquire::Select::new(message, options)
        .with_scorer(&|input, _option, label, _index| score(input, label))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_subsequences_and_ignores_case() {
        assert!(score("qw", "Queue Work").is_some());
        assert!(score("QW", "queue work").is_some());
        assert!(score("wq", "Queue Work").is_none());
        assert!(score("zzz", "Queue Work").is_none());
    }

    #[test]
    fn empty_query_keeps_original_order() {
        assert_eq!(score("", "Queue Work"), Some(0));
        assert_eq!(score("", "Show"), Some(0));
    }

    #[test]
    fn contiguous_matches_rank_above_gaps() {
        assert!(score("stat", "status") > score("stat", "s-t-a-t-u-s"));
    }

    #[test]
    fn punctuation_is_literal_and_unicode_is_supported() {
        assert!(score("^", "queue work").is_none());
        assert!(score("^", "^queue work").is_some());
        assert!(score("\u{e9}", "caf\u{e9}").is_some());
    }

    #[test]
    fn picker_uses_the_fuzzy_scorer() {
        let picker = select("Action", vec!["Queue Work"]);
        assert!((picker.scorer)("qw", &picker.options[0], "Queue Work", 0).is_some());
    }
}
