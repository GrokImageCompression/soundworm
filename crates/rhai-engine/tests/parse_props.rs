// Property tests: compiling and evaluating rhai scripts must never
// panic or hang, only return Ok/Err. Complements the /fuzz rhai target.

use proptest::prelude::*;
use soundworm_core::node::{Node, NodeId, NodeKind};
use soundworm_rhai::ScriptEngine;
use std::collections::HashMap;

fn sample_node() -> Node {
    Node {
        id: NodeId(1),
        name: "n".into(),
        kind: NodeKind::Source,
        app_name: None,
        media_class: "Audio/Source".into(),
        sample_rate: 48000,
        channels: 2,
        latency_ms: 0.0,
        properties: HashMap::new(),
    }
}

proptest! {
    // Arbitrary text into the compiler: never panic.
    #[test]
    fn load_str_never_panics(s in "[\\s\\S]{0,200}") {
        let _ = ScriptEngine::load_str(&s);
    }

    // Anything that compiles must also evaluate without panicking or
    // hanging; the runtime op-limit guard bounds runaway control flow.
    #[test]
    fn evaluate_never_panics(expr in "[a-z0-9_ +*/%()-]{0,40}") {
        let script = format!("let x = {expr};\nallow()");
        if let Ok(engine) = ScriptEngine::load_str(&script) {
            let _ = engine.evaluate(&sample_node(), &["sink".to_string()]);
        }
    }
}
