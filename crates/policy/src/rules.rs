use serde::{Deserialize, Serialize};
use toml;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingRule {
    pub name:     String,
    pub priority: i32,
    pub matches:  MatchCriteria,
    pub action:   Action,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchCriteria {
    pub node_name: Option<String>,
    pub node_kind: Option<String>,
    pub property:  Option<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Action {
    Route     { target: String },
    SetVolume { volume: f32 },
    Deny,
    Notify    { message: String },
}

#[derive(Default)]
pub struct RulesEngine { rules: Vec<RoutingRule> }

impl RulesEngine {
    pub fn load_toml(&mut self, content: &str) -> anyhow::Result<()> {
        #[derive(Deserialize)]
        struct RuleFile { rules: Vec<RoutingRule> }
        let file: RuleFile = toml::from_str(content)?;
        self.rules.extend(file.rules);
        self.rules.sort_by(|a, b| b.priority.cmp(&a.priority));
        Ok(())
    }

    pub fn evaluate(&self, node_name: &str) -> Option<&Action> {
        self.rules.iter().find(|r| {
            r.matches.node_name.as_deref() == Some(node_name)
        }).map(|r| &r.action)
    }

    pub fn rule_count(&self) -> usize { self.rules.len() }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[[rules]]
name     = "spotify"
priority = 10
[rules.matches]
node_name = "spotify"
[rules.action]
Route = { target = "speakers" }
"#;

    #[test]
    fn test_load_and_evaluate() {
        let mut e = RulesEngine::default();
        e.load_toml(SAMPLE).unwrap();
        assert!(matches!(e.evaluate("spotify"), Some(Action::Route { .. })));
    }

    #[test]
    fn test_no_match() {
        let mut e = RulesEngine::default();
        e.load_toml(SAMPLE).unwrap();
        assert!(e.evaluate("unknown").is_none());
    }
}
