use anyhow::{Context, Result};
use soundworm_policy::session::SessionSnapshot;
use std::path::PathBuf;

pub fn snapshot_dir() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            PathBuf::from(home).join(".local/share")
        });
    base.join("soundworm/snapshots")
}

fn snapshot_path(name: &str) -> PathBuf {
    snapshot_dir().join(format!("{}.json", name))
}

pub async fn save(snapshot: &SessionSnapshot) -> Result<()> {
    let dir = snapshot_dir();
    tokio::fs::create_dir_all(&dir).await
        .with_context(|| format!("create snapshot dir {:?}", dir))?;
    let path = snapshot_path(&snapshot.name);
    let json = serde_json::to_string_pretty(snapshot)?;
    tokio::fs::write(&path, json).await
        .with_context(|| format!("write snapshot {:?}", path))
}

pub async fn load(name: &str) -> Result<SessionSnapshot> {
    let path = snapshot_path(name);
    let json = tokio::fs::read_to_string(&path).await
        .with_context(|| format!("read snapshot {:?}", path))?;
    Ok(serde_json::from_str(&json)?)
}

pub async fn list() -> Result<Vec<String>> {
    let dir = snapshot_dir();
    if !dir.exists() { return Ok(vec![]); }
    let mut entries = tokio::fs::read_dir(&dir).await?;
    let mut names = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                names.push(stem.to_owned());
            }
        }
    }
    names.sort();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundworm_policy::session::SessionSnapshot;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_round_trip() {
        let snap = SessionSnapshot {
            name: "test_snap_v01".to_string(),
            timestamp: 99999,
            links: vec![(1, 2), (3, 4)],
            volumes: HashMap::from([(1u64, 0.8f32)]),
        };
        save(&snap).await.unwrap();
        let loaded = load("test_snap_v01").await.unwrap();
        assert_eq!(loaded.name, "test_snap_v01");
        assert_eq!(loaded.links.len(), 2);

        // cleanup
        let path = snapshot_path("test_snap_v01");
        let _ = tokio::fs::remove_file(path).await;
    }
}
