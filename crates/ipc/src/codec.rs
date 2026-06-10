//! NDJSON framing helpers — one `Message` per `\n`-terminated line.
//!
//! Stays sync + transport-agnostic. The daemon wraps these with tokio's
//! `BufReader::read_line` / `AsyncWriteExt::write_all`.

use crate::Message;

pub const MAX_LINE_BYTES: usize = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("line exceeds {MAX_LINE_BYTES} bytes")]
    LineTooLong,
    #[error("invalid json: {0}")]
    Json(#[from] serde_json::Error),
}

pub fn encode(msg: &Message) -> Result<String, CodecError> {
    let mut s = serde_json::to_string(msg)?;
    if s.len() + 1 > MAX_LINE_BYTES {
        return Err(CodecError::LineTooLong);
    }
    s.push('\n');
    Ok(s)
}

pub fn decode(line: &str) -> Result<Message, CodecError> {
    if line.len() > MAX_LINE_BYTES {
        return Err(CodecError::LineTooLong);
    }
    Ok(serde_json::from_str(line.trim_end_matches('\n'))?)
}
