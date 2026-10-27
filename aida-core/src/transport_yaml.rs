//! YAML source grammar as libyaml parses it: events, not bytes. Prose may
//! carry '&', '*' and '!' freely; only real anchors, aliases and tags refuse.
// trace:STORY-1742 | ai:claude
use super::Refusal;
use std::ffi::CStr;
use std::mem::MaybeUninit;
use unsafe_libyaml as sys;

/// Which explicit node tags a document may carry.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tags {
    /// Core-schema tags only (`!!str` and friends).
    Core,
    /// Core tags plus serde's `!Variant` enum encoding (the native oplog).
    /// Variant names are typed by the deserializer, never expanded.
    Variants,
}

const CORE: &[&str] = &["str", "int", "float", "bool", "null", "seq", "map"];

struct Parser(Box<MaybeUninit<sys::yaml_parser_t>>);

impl Drop for Parser {
    fn drop(&mut self) {
        // SAFETY: constructed only after a successful initialize.
        unsafe { sys::yaml_parser_delete(self.0.as_mut_ptr()) }
    }
}

fn node(anchor: *const u8, tag: *const u8, tags: Tags) -> Result<(), Refusal> {
    if !anchor.is_null() {
        return Err(Refusal::Unsupported);
    }
    if tag.is_null() {
        return Ok(());
    }
    // SAFETY: libyaml tags are NUL-terminated and live until event deletion.
    let tag = unsafe { CStr::from_ptr(tag.cast()) }
        .to_str()
        .map_err(|_| Refusal::Encoding)?;
    let core = tag
        .strip_prefix("tag:yaml.org,2002:")
        .is_some_and(|name| CORE.contains(&name));
    let variant = tags == Tags::Variants
        && tag.strip_prefix('!').is_some_and(|name| {
            name.len() <= 64
                && name.starts_with(|c: char| c.is_ascii_alphabetic())
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        });
    if core || variant {
        Ok(())
    } else {
        Err(Refusal::Unsupported)
    }
}

/// Refuse anchors, aliases, tag directives, multiple documents and any tag
/// outside `tags`, by parsing events before any model reader sees the bytes.
/// Aliases are what let a small source expand without bound; tags select
/// non-data constructors. Scalar content is never inspected here.
pub fn plain(bytes: &[u8], tags: Tags) -> Result<(), Refusal> {
    let mut raw = Box::new(MaybeUninit::<sys::yaml_parser_t>::uninit());
    // SAFETY: initialize fully writes the parser before any other use.
    if unsafe { sys::yaml_parser_initialize(raw.as_mut_ptr()) }.fail {
        return Err(Refusal::Limit);
    }
    let mut parser = Parser(raw);
    let p = parser.0.as_mut_ptr();
    // SAFETY: `bytes` outlives `parser`, which is dropped at function exit.
    unsafe {
        sys::yaml_parser_set_encoding(p, sys::YAML_UTF8_ENCODING);
        sys::yaml_parser_set_input_string(p, bytes.as_ptr(), bytes.len() as u64);
    }
    let mut documents = 0usize;
    loop {
        let mut event = MaybeUninit::<sys::yaml_event_t>::uninit();
        // SAFETY: a successful parse fully writes `event`.
        if unsafe { sys::yaml_parser_parse(p, event.as_mut_ptr()) }.fail {
            return Err(Refusal::Encoding);
        }
        // SAFETY: the union member read matches the event type.
        let (verdict, end) = unsafe {
            let e = &*event.as_ptr();
            match e.type_ {
                sys::YAML_STREAM_END_EVENT => (Ok(()), true),
                sys::YAML_DOCUMENT_START_EVENT => {
                    documents += 1;
                    let directives = e.data.document_start.tag_directives;
                    if documents > 1 || directives.start != directives.end {
                        (Err(Refusal::Unsupported), false)
                    } else {
                        (Ok(()), false)
                    }
                }
                sys::YAML_ALIAS_EVENT => (Err(Refusal::Unsupported), false),
                sys::YAML_SCALAR_EVENT => {
                    (node(e.data.scalar.anchor, e.data.scalar.tag, tags), false)
                }
                sys::YAML_SEQUENCE_START_EVENT => (
                    node(
                        e.data.sequence_start.anchor,
                        e.data.sequence_start.tag,
                        tags,
                    ),
                    false,
                ),
                sys::YAML_MAPPING_START_EVENT => (
                    node(e.data.mapping_start.anchor, e.data.mapping_start.tag, tags),
                    false,
                ),
                _ => (Ok(()), false),
            }
        };
        // SAFETY: `event` was written by the successful parse above.
        unsafe { sys::yaml_event_delete(event.as_mut_ptr()) };
        verdict?;
        if end {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn story_1742_yaml_grammar_refuses_structure_not_prose_bytes() {
        for prose in [
            "title: '!'\nlabel: '!!'\n",
            "description: \"*this is bold* & **strong**\"\n",
            "description: Borders & Rounding *x* !y\n",
            "items:\n  - Spacing & Layout\n  - a*b!c\n",
            "description: |\n  &anchor-looking *alias-looking !tag-looking\n",
            "n: !!str 5\n",
        ] {
            plain(prose.as_bytes(), Tags::Core).unwrap();
        }
        for structure in [
            "a: &x [1]\nb: *x\n",
            "a: &x 1\n",
            "a: !Create {title: t}\n",
            "a: !!binary aGk=\n",
            "a: !<tag:example.invalid,2000:x> 1\n",
            "%TAG ! tag:yaml.org,2002:\n---\na: !str 1\n",
            "a: 1\n---\nb: 2\n",
        ] {
            assert_eq!(
                plain(structure.as_bytes(), Tags::Core),
                Err(Refusal::Unsupported),
                "{structure}"
            );
        }
        plain(b"kind: !Create\n  title: t\n", Tags::Variants).unwrap();
        assert!(plain(b"kind: !Cre-ate 1\n", Tags::Variants).is_err());
        assert_eq!(plain(b"a: [1\n", Tags::Core), Err(Refusal::Encoding));
    }
}
