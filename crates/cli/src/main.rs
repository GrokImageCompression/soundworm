mod commands;
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".into()))
        .init();

    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()).unwrap_or("help") {
        "list"     => commands::list::run().await,
        "link"     => commands::link::run(&args).await,
        "unlink"   => commands::link::unlink(&args).await,
        "snapshot" => commands::snapshot::run(&args).await,
        "metrics"  => commands::metrics::run().await,
        _ => {
            println!("soundworm (sw) — cross-platform audio router");
            println!();
            println!("USAGE:");
            println!("  sw list                     List audio nodes");
            println!("  sw link   <src> <sink>      Create route");
            println!("  sw unlink <link-id>         Remove route");
            println!("  sw snapshot save <name>     Save session");
            println!("  sw snapshot load <name>     Restore session");
            println!("  sw snapshot list            List sessions");
            println!("  sw metrics                  Latency + xrun stats");
            println!();
            println!("ENV:  RUST_LOG=debug sw ...");
            Ok(())
        }
    }
}
