use anyhow::Result;
use std::path::Path;
use soundworm_policy::session::SessionSnapshot;

pub async fn save(snapshot: &SessionSnapshot, path: &Path) -> Result<()> {
    let json = serde_json::to_string_pretty(snapshot)?;
    tokio::fs::write(path, json).await?;
    Ok(())
}

pub async fn load(path: &Path) -> Result<SessionSnapshot> {
    let json = tokio::fs::read_to_string(path).await?;
    Ok(serde_json::from_str(&json)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundworm_policy::session::SessionSnapshot;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_round_trip() {
        let snap = SessionSnapshot {
            name: "test".to_string(), timestamp: 12345,
            links: vec![(1, 2), (3, 4)],
            volumes: HashMap::from([(1u64, 0.8f32)]),
        };
        let path = std::env::temp_dir().join("soundworm_test.json");
        save(&snap, &path).await.unwrap();
        let loaded = load(&path).await.unwrap();
        assert_eq!(loaded.name, "test");
        assert_eq!(loaded.links.len(), 2);
        tokio::fs::remove_file(&path).await.unwrap();
    }
}
