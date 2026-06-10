use anyhow::Result;
pub async fn run(args: &[String]) -> Result<()> {
    let src  = args.get(2).map(|s| s.as_str()).unwrap_or("<src>");
    let sink = args.get(3).map(|s| s.as_str()).unwrap_or("<sink>");
    println!("Linking: {} → {}", src, sink);
    Ok(())
}
pub async fn unlink(args: &[String]) -> Result<()> {
    let id = args.get(2).map(|s| s.as_str()).unwrap_or("<id>");
    println!("Removing link: {}", id);
    Ok(())
}
