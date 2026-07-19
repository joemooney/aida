use super::sh_single_quote;

// The exact purpose text that broke `aida role enter product` (BUG-427):
// apostrophes in `that's` / `'advisor'` closed the quote and exposed the
// parens to bare bash. Wrapped in single quotes after escaping, it must be
// a single well-formed shell word with the original text recovered.
#[test]
fn escaped_purpose_is_a_single_well_formed_shell_word() {
    let purpose = "do NOT drive execution (that's the 'advisor' seat)";
    let wrapped = format!("'{}'", sh_single_quote(purpose));
    // Round-trip: the only way the escaping is correct is if splitting on
    // the close-quote/reopen-quote boundaries reconstructs the original.
    // A POSIX shell parses '...'\''...' as one concatenated word == purpose.
    let reconstructed = wrapped
        .trim_start_matches('\'')
        .trim_end_matches('\'')
        .replace("'\\''", "'");
    assert_eq!(reconstructed, purpose);
}

#[test]
fn no_apostrophe_is_identity() {
    let s = "Intake / product advisor — clean text, no quotes";
    assert_eq!(sh_single_quote(s), s);
}

#[test]
fn each_apostrophe_becomes_the_four_char_escape() {
    assert_eq!(sh_single_quote("a'b"), "a'\\''b");
    assert_eq!(sh_single_quote("''"), "'\\'''\\''");
}
