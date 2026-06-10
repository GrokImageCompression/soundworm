use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SessionSnapshot {
    pub name:      String,
    pub timestamp: u64,
    pub links:     Vec<(u64, u64)>,
    pub volumes:   HashMap<u64, f32>,
}

#[derive(Default)]
pub struct SessionManager { snapshots: Vec<SessionSnapshot> }

impl SessionManager {
    pub fn new() -> Self { Self::default() }

    pub fn save(&mut self, snapshot: SessionSnapshot) {
        self.snapshots.retain(|s| s.name != snapshot.name);
        self.snapshots.push(snapshot);
    }

    pub fn load(&self, name: &str) -> Option<&SessionSnapshot> {
        self.snapshots.iter().find(|s| s.name == name)
    }

    pub fn list(&self) -> Vec<&str> {
        self.snapshots.iter().map(|s| s.name.as_str()).collect()
    }
}
