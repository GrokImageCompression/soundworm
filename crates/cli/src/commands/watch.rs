use anyhow::Result;
use soundworm_ipc::{client::connect_subscriber, default_socket_path, Event};

pub async fn run() -> Result<()> {
    let mut events = connect_subscriber(&default_socket_path(), None).await?;
    eprintln!("Listening for events (ctrl-c to quit)…");
    while let Some(ev) = events.recv().await {
        match ev {
            Event::NodeAppeared { node } => {
                println!("+ node {} {}", node.id.0, node.name)
            }
            Event::NodeRemoved { node_id } => println!("- node {}", node_id.0),
            Event::LinkAppeared { link } => println!(
                "+ link {} ({} → {})",
                link.id.0, link.source_port.0, link.sink_port.0
            ),
            Event::LinkRemoved { link_id } => println!("- link {}", link_id.0),
            Event::RulesApplied { rule, link_id } => {
                println!("rule '{}' linked {}", rule, link_id.0)
            }
            Event::LinkRejected { reason } => println!("rejected: {}", reason),
            Event::EventsDropped { count } => eprintln!("⚠ dropped {} events", count),
        }
    }
    Ok(())
}
