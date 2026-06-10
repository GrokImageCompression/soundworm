use soundworm_core::node::NodeId;

#[derive(Debug)]
pub struct Conflict {
    pub device:    String,
    pub claimants: Vec<NodeId>,
}

pub enum Resolution { FirstWins, PriorityWins, UserPrompt }

pub struct ConflictResolver { pub strategy: Resolution }

impl ConflictResolver {
    pub fn resolve(&self, conflict: &Conflict) -> Option<NodeId> {
        match self.strategy {
            Resolution::FirstWins    => conflict.claimants.first().cloned(),
            Resolution::PriorityWins => conflict.claimants.last().cloned(),
            Resolution::UserPrompt   => {
                tracing::warn!("Conflict on '{}' — user prompt not implemented", conflict.device);
                None
            }
        }
    }
}
