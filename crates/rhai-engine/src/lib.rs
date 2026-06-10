use anyhow::Result;
use rhai::{Engine, Scope};

pub struct ScriptEngine { engine: Engine }

impl ScriptEngine {
    pub fn new() -> Self {
        let mut engine = Engine::new();
        engine.register_fn("log_route", |from: &str, to: &str| {
            println!("[soundworm] route: {} → {}", from, to);
        });
        engine.register_fn("allow", || true);
        engine.register_fn("deny",  || false);
        Self { engine }
    }

    pub fn eval_routing_script(&self, script: &str, node_name: &str) -> Result<bool> {
        let mut scope = Scope::new();
        scope.push("node_name", node_name.to_string());
        let result: bool = self.engine
            .eval_with_scope(&mut scope, script)
            .map_err(|e| anyhow::anyhow!("Script error: {}", e))?;
        Ok(result)
    }
}

impl Default for ScriptEngine { fn default() -> Self { Self::new() } }

#[cfg(test)]
mod tests {
    use super::*;
    const SCRIPT: &str = r#"if node_name == "spotify" { allow() } else { deny() }"#;

    #[test]
    fn test_allow() {
        assert!(ScriptEngine::new().eval_routing_script(SCRIPT, "spotify").unwrap());
    }
    #[test]
    fn test_deny() {
        assert!(!ScriptEngine::new().eval_routing_script(SCRIPT, "zoom").unwrap());
    }
}
