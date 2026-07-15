use anyhow::{anyhow, Result};
use comfy_table::{presets::UTF8_FULL, Table};
#[cfg(target_os = "linux")]
use soundworm_core::backend::AudioBackend;
use soundworm_ipc::{client::Client, default_socket_path, Op, ResponseData};
#[cfg(target_os = "linux")]
use soundworm_pipewire::PipeWireBackend;

pub async fn run(in_process: bool) -> Result<()> {
    let nodes = if in_process {
        #[cfg(target_os = "linux")]
        { PipeWireBackend::new()?.enumerate_nodes().await? }
        #[cfg(not(target_os = "linux"))]
        { return Err(anyhow!("--in-process is Linux-only; use the daemon")); }
    } else {
        let mut c = Client::connect(&default_socket_path()).await?;
        match c.request(Op::ListNodes).await? {
            // Daemon now returns NodeView (Node + embedded ports); the
            // CLI table only renders the Node fields, so project away
            // the ports here.
            ResponseData::Nodes { nodes } => nodes.into_iter().map(|nv| nv.node).collect(),
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
