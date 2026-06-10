use anyhow::Result;
pub async fn run(args: &[String]) -> Result<()> {
    let sub  = args.get(2).map(|s| s.as_str()).unwrap_or("list");
    let name = args.get(3).map(|s| s.as_str()).unwrap_or("default");
    match sub {
        "save" => println!("Saving snapshot '{}'", name),
        "load" => println!("Loading snapshot '{}'", name),
        _      => println!("No saved snapshots yet"),
    }
    Ok(())
}
