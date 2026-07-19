# aida-cli-lib

Implementation library for the [`aida`](https://github.com/joemooney/aida) CLI.
The `aida-cli` crate's binary is a thin stub over this crate's one public entry
point, `main_entry()` — keeping the whole CLI (dispatch, handlers, and their
tests) inside a library target so the code is testable as a library and the
entry-point boundary stays clean.

This crate is an internal implementation detail of the `aida` toolchain; its
API surface is deliberately a single function and carries no stability promise.
Install the CLI itself instead:

```bash
cargo install aida-cli
```
