use super::IniError;

const UTF8_BOM: &[u8] = &[0xef, 0xbb, 0xbf];
const UTF16LE_BOM: &[u8] = &[0xff, 0xfe];
const UTF16BE_BOM: &[u8] = &[0xfe, 0xff];

#[derive(Clone, Copy, Debug)]
pub(crate) enum Encoding {
    Utf8 { bom: bool },
    Utf16Le,
}

impl Encoding {
    pub(crate) fn decode(bytes: &[u8]) -> Result<(Self, String), IniError> {
        if let Some(payload) = bytes.strip_prefix(UTF16LE_BOM) {
            if payload.len() % 2 != 0 {
                return Err(IniError::UnsupportedEncoding);
            }

            let units = payload
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect::<Vec<_>>();
            let text = String::from_utf16(&units).map_err(|_| IniError::UnsupportedEncoding)?;
            return Ok((Self::Utf16Le, text));
        }

        if bytes.starts_with(UTF16BE_BOM) {
            return Err(IniError::UnsupportedEncoding);
        }

        let (bom, payload) = match bytes.strip_prefix(UTF8_BOM) {
            Some(payload) => (true, payload),
            None => (false, bytes),
        };
        if payload.contains(&0) {
            return Err(IniError::UnsupportedEncoding);
        }

        let text = std::str::from_utf8(payload)
            .map_err(|_| IniError::UnsupportedEncoding)?
            .to_owned();
        Ok((Self::Utf8 { bom }, text))
    }

    pub(crate) fn encode(self, text: &str) -> Vec<u8> {
        match self {
            Self::Utf8 { bom } => {
                let mut bytes = Vec::with_capacity(text.len() + usize::from(bom) * 3);
                if bom {
                    bytes.extend_from_slice(UTF8_BOM);
                }
                bytes.extend_from_slice(text.as_bytes());
                bytes
            }
            Self::Utf16Le => {
                let mut bytes = Vec::with_capacity(text.len() * 2 + 2);
                bytes.extend_from_slice(UTF16LE_BOM);
                for unit in text.encode_utf16() {
                    bytes.extend_from_slice(&unit.to_le_bytes());
                }
                bytes
            }
        }
    }
}
