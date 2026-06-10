use anyhow::{anyhow, Result};
use comfy_table::{presets::UTF8_FULL, Table};
use soundworm_core::backend::AudioBackend;
use soundworm_ipc::{client::Client, default_socket_path, Op, ResponseData};
use soundworm_pipewire::PipeWireBackend;

pub async fn run(in_process: bool) -> Result<()> {
    let nodes = if in_process {
        PipeWireBackend::new()?.enumerate_nodes().await?
    } else {
        let mut c = Client::connect(&default_socket_path()).await?;
        match c.request(Op::ListNodes).await? {
            ResponseData::Nodes { nodes } => nodes,
            _ => return Err(anyhow!("unexpected response from daemon")),
        }
    };

    if nodes.is_empty() {
        println!("No audio nodes found.");
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header(["ID", "Name", "App", "Media Class", "Kind"]);
    for n in &nodes {
        table.add_row([
            n.id.0.to_string(),
            n.name.clone(),
            n.app_name.clone().unwrap_or_default(),
            n.media_class.clone(),
            format!("{:?}", n.kind),
        ]);
    }
    println!("{table}");
    Ok(())
}
