mod commands;
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".into()))
        .init();

    let raw: Vec<String> = std::env::args().collect();
    let in_process = raw.iter().any(|a| a == "--in-process");
    let args: Vec<String> = raw.into_iter().filter(|a| a != "--in-process").collect();

    match args.get(1).map(|s| s.as_str()).unwrap_or("help") {
        "list"     => commands::list::run(in_process).await,
        "link"     => commands::link::run(&args, in_process).await,
        "unlink"   => commands::link::unlink(&args, in_process).await,
        "watch"    => commands::watch::run().await,
        "snapshot" => commands::snapshot::run(&args).await,
        "metrics"  => commands::metrics::run().await,
        _ => {
            println!("soundworm (sw) — cross-platform audio router");
            println!();
            println!("USAGE:");
            println!("  sw list                     List audio nodes (via daemon)");
            println!("  sw link   <src> <sink>      Create route (via daemon)");
            println!("  sw unlink <link-id>         Remove route (via daemon)");
            println!("  sw watch                    Stream live events from daemon");
            println!("  sw snapshot save <name>     Save session");
            println!("  sw snapshot load <name>     Restore session");
            println!("  sw snapshot list            List sessions");
            println!("  sw metrics                  Latency + xrun stats");
            println!();
            println!("FLAGS:");
            println!("  --in-process                Bypass daemon, talk to backend directly");
            println!("                              (test escape hatch — list/link/unlink only)");
            println!();
            println!("ENV:  RUST_LOG=debug sw ...   SOUNDWORM_SOCK=<path> sw ...");
            Ok(())
        }
    }
}
