//! Terminal rendering, telemetry and process utilities independent of CLI parsing.
// trace:TASK-3783 | ai:codex
macro_rules! print {
    ($($arg:tt)*) => { $crate::cache_output::print_tracked(::std::format_args!($($arg)*)) };
}
macro_rules! println {
    () => { $crate::cache_output::println_tracked(::std::format_args!("")) };
    ($($arg:tt)*) => { $crate::cache_output::println_tracked(::std::format_args!($($arg)*)) };
}
pub mod cache_output;
pub mod git_arg_guard;
pub mod glyph_config;
pub mod glyphs;
pub mod network_retry;
pub mod output_format;
pub mod process_retry;
pub mod toon;
pub mod usage;
