// trace:TASK-3325 | ai:codex
//! Shared TypeScript reflection settings for schema output and web bindings.

use ts_rs::{Config, TS};

/// Reflect a declaration with the existing optional-map and bigint contract.
pub fn declaration<T: TS>() -> String {
    // v12 changed map keys to required. Keep the existing v11 wire contract.
    #[allow(deprecated)]
    let config = Config::default().with_v11_hashmap();
    T::decl(&config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(TS)]
    struct Contract {
        values: HashMap<String, String>,
        count: u64,
    }

    #[test]
    fn map_keys_and_large_integers_keep_existing_types() {
        let output = declaration::<Contract>();
        assert!(output.contains("[key in string]?: string"), "{output}");
        assert!(output.contains("count: bigint"), "{output}");
    }
}
