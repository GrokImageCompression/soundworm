use anyhow::{anyhow, Result};
use soundworm_core::backend::AudioBackend;
use soundworm_pipewire::PipeWireBackend;
use soundworm_policy::session::SessionSnapshot;
use soundworm_snapshots as snapshots;
use std::time::{SystemTime, UNIX_EPOCH};

pub async fn run(args: &[String]) -> Result<()> {
    let sub  = args.get(2).map(|s| s.as_str()).unwrap_or("list");
    let name = args.get(3).map(|s| s.as_str()).unwrap_or("default");

    match sub {
        "save" => {
            let backend = PipeWireBackend::new()?;
            let _nodes = backend.enumerate_nodes().await?;
            // Capture current links by subscribing briefly.
            let rx = backend.subscribe();
            let mut links: Vec<(u64, u64)> = Vec::new();
            while let Ok(evt) = rx.try_recv() {
                use soundworm_core::event::BackendEvent;
                if let BackendEvent::LinkAppeared(l) = evt {
                    links.push((l.source_port.0, l.sink_port.0));
                }
            }
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH).unwrap().as_secs();
            let snap = SessionSnapshot {
                name: name.to_owned(),
                timestamp,
                links,
                volumes: std::collections::HashMap::new(),
            };
            snapshots::save(&snap).await?;
            println!("Saved snapshot '{}'", name);
        }
        "load" => {
            let snap = snapshots::load(name).await?;
            let backend = PipeWireBackend::new()?;
            let mut restored = 0usize;
            for (src_port, dst_port) in &snap.links {
                let link = soundworm_core::link::Link {
                    id: soundworm_core::link::LinkId(0),
                    source_port: soundworm_core::port::PortId(*src_port),
                    sink_port:   soundworm_core::port::PortId(*dst_port),
                    latency_compensation_ms: 0.0,
                };
                if backend.create_link(&link).await.is_ok() { restored += 1; }
            }
            println!("Restored snapshot '{}': {} link(s)", name, restored);
        }
        "list" => {
            let names = snapshots::list().await?;
            if names.is_empty() {
                println!("No saved snapshots.");
            } else {
                for n in names { println!("  {}", n); }
            }
        }
        other => return Err(anyhow!("Unknown snapshot subcommand '{}'", other)),
    }
    Ok(())
}
