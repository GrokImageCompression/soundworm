use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct NodeId(pub u64);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NodeKind { Source, Sink, Filter, Virtual }

/// Stream media kind, used to decide whether two ports can be linked
/// together. PipeWire's link factory accepts the create_object call
/// even for incompatible types (Audio ↔ MIDI), then silently destroys
/// the link a few ms later — so we have to gate it ourselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaKind { Audio, Midi, Video, Other }

impl MediaKind {
    /// Derive the kind from PipeWire's `media.class` string
    /// ("Audio/Sink", "Stream/Output/Audio", "Midi/Source", …).
    /// Falls back to `Other` for anything we don't recognize.
    pub fn from_media_class(mc: &str) -> Self {
        let lc = mc.to_ascii_lowercase();
        if lc.contains("midi") { Self::Midi }
        else if lc.contains("audio") { Self::Audio }
        else if lc.contains("video") { Self::Video }
        else { Self::Other }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id:          NodeId,
    pub name:        String,
    pub kind:        NodeKind,
    pub app_name:    Option<String>,
    pub media_class: String,
    pub sample_rate: u32,
    pub channels:    u8,
    pub latency_ms:  f32,
    pub properties:  HashMap<String, String>,
}

impl Node {
    pub fn media_kind(&self) -> MediaKind {
        MediaKind::from_media_class(&self.media_class)
    }

    pub fn new(id: u64, name: impl Into<String>, kind: NodeKind) -> Self {
        Self {
            id: NodeId(id),
            name: name.into(),
            kind,
            app_name: None,
            media_class: String::new(),
            sample_rate: 48000,
            channels: 2,
            latency_ms: 0.0,
            properties: HashMap::new(),
        }
    }
}
