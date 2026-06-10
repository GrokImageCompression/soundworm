use anyhow::{anyhow, Result};
use soundworm_core::{backend::AudioBackend, link::Link, link::LinkId, port::PortId};
use soundworm_graph::AudioGraph;
use soundworm_pipewire::PipeWireBackend;

pub async fn run(args: &[String]) -> Result<()> {
    let src_name  = args.get(2).ok_or_else(|| anyhow!("Usage: sw link <src-node> <sink-node>"))?;
    let sink_name = args.get(3).ok_or_else(|| anyhow!("Usage: sw link <src-node> <sink-node>"))?;

    let backend = PipeWireBackend::new()?;
    let nodes = backend.enumerate_nodes().await?;

    let mut graph = AudioGraph::new();

    // Subscribe for port events that arrive after enumerate.
    let rx = backend.subscribe();
    while let Ok(event) = rx.try_recv() {
        graph.apply_event(event);
    }
    for node in nodes {
        graph.add_node(node);
    }

    let src = graph.find_node_by_name(src_name)
        .ok_or_else(|| anyhow!("Source node '{}' not found", src_name))?;
    let sink = graph.find_node_by_name(sink_name)
        .ok_or_else(|| anyhow!("Sink node '{}' not found", sink_name))?;

    let out_port = graph.output_ports_of(&src.id).into_iter().next()
        .ok_or_else(|| anyhow!("No output port on '{}'", src_name))?;
    let in_port  = graph.input_ports_of(&sink.id).into_iter().next()
        .ok_or_else(|| anyhow!("No input port on '{}'", sink_name))?;

    let link = Link {
        id: LinkId(0),
        source_port: PortId(out_port.id.0),
        sink_port:   PortId(in_port.id.0),
        latency_compensation_ms: 0.0,
    };

    backend.create_link(&link).await?;
    println!("Linked '{}' → '{}'  (port {} → {})", src_name, sink_name, out_port.id.0, in_port.id.0);
    Ok(())
}

pub async fn unlink(args: &[String]) -> Result<()> {
    let id_str = args.get(2).ok_or_else(|| anyhow!("Usage: sw unlink <link-id>"))?;
    let id: u64 = id_str.parse().map_err(|_| anyhow!("Invalid link id: {}", id_str))?;

    let backend = PipeWireBackend::new()?;
    let link = Link {
        id: LinkId(id),
        source_port: PortId(0),
        sink_port: PortId(0),
        latency_compensation_ms: 0.0,
    };
    backend.destroy_link(&link).await?;
    println!("Removed link {}", id);
    Ok(())
}
