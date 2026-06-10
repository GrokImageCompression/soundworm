mod ipc_server;
mod state;

use anyhow::Result;
use soundworm_observability::{metrics::Metrics, xrun::XrunLog};
use soundworm_pipewire::PipeWireBackend;
use soundworm_policy::rules::RulesEngine;
use soundworm_core::backend::AudioBackend;
use state::DaemonState;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()))
        .init();

    tracing::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    tracing::info!(" soundworm daemon starting");
    tracing::info!(" platform: {}", std::env::consts::OS);
    tracing::info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    let backend: Arc<dyn AudioBackend> = Arc::new(PipeWireBackend::new()?);
    let nodes = backend.enumerate_nodes().await?;
    tracing::info!("Backend '{}': {} nodes found", backend.name(), nodes.len());

    let state = Arc::new(DaemonState::new(Arc::clone(&backend)));
    {
        let mut g = state.graph.lock().unwrap();
        for node in nodes {
            g.add_node(node);
        }
    }
    state.start_event_pump();

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

    let sock = ipc_server::socket_path();
    let ipc_state = Arc::clone(&state);
    let ipc = tokio::spawn(async move {
        if let Err(e) = ipc_server::serve(sock, ipc_state).await {
            tracing::error!("IPC server crashed: {e:#}");
        }
    });

    tracing::info!("Ready — ctrl-c to stop");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = ipc => {}
    }
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
