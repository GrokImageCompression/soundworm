use thiserror::Error;

#[derive(Debug, Error)]
pub enum SoundwormError {
    #[error("Node not found: {0}")]
    NodeNotFound(String),
    #[error("Port not found: {0}")]
    PortNotFound(String),
    #[error("Link already exists")]
    LinkExists,
    #[error("Backend error: {0}")]
    Backend(String),
    #[error("Policy error: {0}")]
    Policy(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, SoundwormError>;
