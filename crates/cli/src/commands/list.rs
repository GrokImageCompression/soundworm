use anyhow::Result;
use comfy_table::{Table, presets::UTF8_FULL};
use soundworm_core::backend::AudioBackend;
use soundworm_pipewire::PipeWireBackend;

pub async fn run() -> Result<()> {
    let backend = PipeWireBackend::new()?;
    let nodes = backend.enumerate_nodes().await?;

    if nodes.is_empty() {
        println!("No audio nodes found (is PipeWire running?)");
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
