// Property tests: the rules-TOML parser must never panic on any input,
// only return Ok/Err. Runs in normal `cargo test` (and CI), complementing
// the on-demand cargo-fuzz target in /fuzz.

use proptest::prelude::*;
use soundworm_policy::rules::RulesEngine;

proptest! {
    // Arbitrary bytes-as-text (including newlines/control chars).
    #[test]
    fn load_toml_never_panics(s in "[\\s\\S]{0,200}") {
        let mut engine = RulesEngine::default();
        let _ = engine.load_toml(&s);
    }

    // Schema-shaped input with fuzzed values exercises the deserializer
    // deeper than random noise usually reaches.
    #[test]
    fn load_toml_structured_never_panics(
        name in "[\\x20-\\x7e]{0,40}",
        priority in any::<i32>(),
    ) {
        let doc = format!(
            "[[rules]]\nname = {name:?}\npriority = {priority}\n\
             [rules.matches]\nnode_name = {name:?}\n\
             [rules.action]\nRoute = {{ target = {name:?} }}\n"
        );
        let mut engine = RulesEngine::default();
        let _ = engine.load_toml(&doc);
    }
}
