use std::time::SystemTime;
use soundworm_core::node::NodeId;

#[derive(Debug)]
pub struct Xrun { pub node_id: NodeId, pub timestamp: SystemTime, pub gap_ms: f32 }

#[derive(Default)]
pub struct XrunLog { events: Vec<Xrun> }

impl XrunLog {
    pub fn record(&mut self, node_id: NodeId, gap_ms: f32) {
        tracing::warn!("Xrun on {:?}: {:.2}ms", node_id, gap_ms);
        self.events.push(Xrun { node_id, timestamp: SystemTime::now(), gap_ms });
    }
    pub fn recent(&self, n: usize) -> &[Xrun] {
        let len = self.events.len();
        &self.events[len.saturating_sub(n)..]
    }
    pub fn worst_offender(&self) -> Option<&NodeId> {
        self.events.iter()
            .max_by(|a, b| a.gap_ms.partial_cmp(&b.gap_ms).unwrap())
            .map(|x| &x.node_id)
    }
    pub fn total(&self) -> usize { self.events.len() }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_xrun_log() {
        let mut log = XrunLog::default();
        log.record(NodeId(1), 2.5);
        log.record(NodeId(2), 10.0);
        log.record(NodeId(1), 1.0);
        assert_eq!(log.total(), 3);
        assert_eq!(log.worst_offender(), Some(&NodeId(2)));
    }
}
