mod encoding;
mod error;
mod line;
mod merge;

pub use error::IniError;
pub use merge::{ManagedChange, MergePreview, SemanticChange};

use encoding::Encoding;
use line::Line;

#[derive(Clone, Debug)]
pub struct IniDocument {
    original: Vec<u8>,
    encoding: Encoding,
    lines: Vec<Line>,
}

impl IniDocument {
    pub fn parse(bytes: &[u8]) -> Result<Self, IniError> {
        let (encoding, text) = Encoding::decode(bytes)?;
        Ok(Self {
            original: bytes.to_vec(),
            encoding,
            lines: Line::parse_all(&text),
        })
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.original
    }

    pub fn merge(&self, changes: &[ManagedChange]) -> Result<MergePreview, IniError> {
        merge::merge(&self.original, self.encoding, &self.lines, changes)
    }
}
