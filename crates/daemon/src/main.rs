use anyhow::Result;
use soundworm_graph::AudioGraph;
use soundworm_observability::{metrics::Metrics, xrun::XrunLog};
use soundworm_pipewire::PipeWireBackend;
use soundworm_policy::rules::RulesEngine;
use soundworm_core::backend::AudioBackend;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()))
        .init();

    tracing::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    tracing::info!(" soundworm daemon starting");
    tracing::info!(" platform: {}", std::env::consts::OS);
    tracing::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    let backend = PipeWireBackend::new()?;
    let nodes   = backend.enumerate_nodes().await?;
    tracing::info!("Backend '{}': {} nodes found", backend.name(), nodes.len());

    let _graph   = AudioGraph::new();
    let _xruns   = XrunLog::default();
    let _metrics = Metrics::default();

    let rules_path = config_dir().join("soundworm/rules/default.toml");
    if rules_path.exists() {
        let content = std::fs::read_to_string(&rules_path)?;
        let mut rules = RulesEngine::default();
        rules.load_toml(&content)?;
        tracing::info!("Loaded {} rules from {:?}", rules.rule_count(), rules_path);
    } else {
        tracing::info!("No rules file at {:?} — using defaults", rules_path);
    }

    tracing::info!("Ready — ctrl-c to stop");
    tokio::signal::ctrl_c().await?;
    tracing::info!("Shutdown complete");
    Ok(())
}

fn config_dir() -> std::path::PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let mut p = std::env::var("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"));
            p.push(".config");
            p
        })
}
