use std::{collections::HashSet, path::PathBuf};

use super::DiscoveryError;

pub(crate) const MAX_LIBRARY_FOLDERS_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MAX_APP_MANIFEST_BYTES: usize = 512 * 1024;
const MAX_DEPTH: usize = 16;
const MAX_NODES: usize = 16_384;
const MAX_TOKEN_BYTES: usize = 32 * 1024;

#[derive(Debug)]
enum Value {
    Text(String),
    Object(Vec<(String, Value)>),
}

struct Parser<'a> {
    input: &'a [u8],
    position: usize,
    nodes: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a [u8]) -> Result<Self, DiscoveryError> {
        if input.contains(&0) {
            return Err(DiscoveryError::InvalidKeyValues("nul_byte"));
        }
        Ok(Self {
            input,
            position: 0,
            nodes: 0,
        })
    }

    fn parse(mut self) -> Result<Vec<(String, Value)>, DiscoveryError> {
        let entries = self.parse_entries(false, 0)?;
        self.skip_trivia()?;
        if self.position != self.input.len() {
            return Err(DiscoveryError::InvalidKeyValues("trailing_data"));
        }
        Ok(entries)
    }

    fn parse_entries(
        &mut self,
        expects_closing_brace: bool,
        depth: usize,
    ) -> Result<Vec<(String, Value)>, DiscoveryError> {
        if depth > MAX_DEPTH {
            return Err(DiscoveryError::InvalidKeyValues("nesting_limit"));
        }
        let mut entries = Vec::new();
        let mut keys = HashSet::new();
        loop {
            self.skip_trivia()?;
            match self.peek() {
                None if expects_closing_brace => {
                    return Err(DiscoveryError::InvalidKeyValues("unclosed_object"));
                }
                None => break,
                Some(b'}') if expects_closing_brace => {
                    self.position += 1;
                    break;
                }
                Some(b'}') => {
                    return Err(DiscoveryError::InvalidKeyValues("unexpected_closing_brace"));
                }
                Some(b'{') => {
                    return Err(DiscoveryError::InvalidKeyValues("missing_key"));
                }
                Some(b'\"') => {}
                Some(_) => return Err(DiscoveryError::InvalidKeyValues("unquoted_key")),
            }

            let key = self.parse_quoted()?;
            if key.is_empty() {
                return Err(DiscoveryError::InvalidKeyValues("empty_key"));
            }
            if !keys.insert(key.to_ascii_lowercase()) {
                return Err(DiscoveryError::InvalidKeyValues("duplicate_key"));
            }
            self.skip_trivia()?;
            let value = match self.peek() {
                Some(b'\"') => Value::Text(self.parse_quoted()?),
                Some(b'{') => {
                    self.position += 1;
                    Value::Object(self.parse_entries(true, depth + 1)?)
                }
                _ => return Err(DiscoveryError::InvalidKeyValues("missing_value")),
            };
            self.nodes += 1;
            if self.nodes > MAX_NODES {
                return Err(DiscoveryError::InvalidKeyValues("node_limit"));
            }
            entries.push((key, value));
        }
        Ok(entries)
    }

    fn parse_quoted(&mut self) -> Result<String, DiscoveryError> {
        if self.peek() != Some(b'\"') {
            return Err(DiscoveryError::InvalidKeyValues("expected_quote"));
        }
        self.position += 1;
        let mut output = Vec::new();
        loop {
            let byte = self
                .peek()
                .ok_or(DiscoveryError::InvalidKeyValues("unclosed_string"))?;
            self.position += 1;
            match byte {
                b'\"' => break,
                b'\\' => {
                    let escaped = self
                        .peek()
                        .ok_or(DiscoveryError::InvalidKeyValues("unclosed_escape"))?;
                    self.position += 1;
                    match escaped {
                        b'\\' | b'\"' | b'/' => output.push(escaped),
                        _ => return Err(DiscoveryError::InvalidKeyValues("unsupported_escape")),
                    }
                }
                b'\r' | b'\n' => {
                    return Err(DiscoveryError::InvalidKeyValues("newline_in_string"));
                }
                _ => output.push(byte),
            }
            if output.len() > MAX_TOKEN_BYTES {
                return Err(DiscoveryError::InvalidKeyValues("token_limit"));
            }
        }
        String::from_utf8(output).map_err(|_| DiscoveryError::InvalidKeyValues("invalid_utf8"))
    }

    fn skip_trivia(&mut self) -> Result<(), DiscoveryError> {
        loop {
            while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
                self.position += 1;
            }
            if self.peek() == Some(b'/') && self.input.get(self.position + 1) == Some(&b'/') {
                self.position += 2;
                while let Some(byte) = self.peek() {
                    self.position += 1;
                    if byte == b'\n' {
                        break;
                    }
                }
                continue;
            }
            if self.peek() == Some(b'/') {
                return Err(DiscoveryError::InvalidKeyValues("invalid_comment"));
            }
            return Ok(());
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.position).copied()
    }
}

fn parse_bounded(input: &[u8], maximum: usize) -> Result<Vec<(String, Value)>, DiscoveryError> {
    if input.len() > maximum {
        return Err(DiscoveryError::InputTooLarge {
            actual: input.len(),
            maximum,
        });
    }
    Parser::new(input)?.parse()
}

pub(crate) fn parse_library_folders(input: &[u8]) -> Result<Vec<PathBuf>, DiscoveryError> {
    let document = parse_bounded(input, MAX_LIBRARY_FOLDERS_BYTES)?;
    let [(root_name, Value::Object(libraries))] = document.as_slice() else {
        return Err(DiscoveryError::InvalidKeyValues("libraryfolders_root"));
    };
    if !root_name.eq_ignore_ascii_case("libraryfolders") {
        return Err(DiscoveryError::InvalidKeyValues("libraryfolders_root"));
    }

    let mut paths = Vec::new();
    for (index, value) in libraries {
        if index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let raw_path = match value {
            Value::Text(path) => path,
            Value::Object(properties) => properties
                .iter()
                .find_map(|(key, value)| {
                    (key.eq_ignore_ascii_case("path"))
                        .then_some(value)
                        .and_then(|value| match value {
                            Value::Text(path) => Some(path),
                            Value::Object(_) => None,
                        })
                })
                .ok_or(DiscoveryError::InvalidKeyValues("library_path"))?,
        };
        if raw_path.is_empty() || raw_path.len() > MAX_TOKEN_BYTES {
            return Err(DiscoveryError::InvalidKeyValues("library_path"));
        }
        let path = PathBuf::from(raw_path);
        if !path.is_absolute() {
            return Err(DiscoveryError::InvalidKeyValues("relative_library_path"));
        }
        paths.push(path);
    }
    Ok(paths)
}

pub(crate) fn parse_app_manifest(input: &[u8]) -> Result<String, DiscoveryError> {
    let document = parse_bounded(input, MAX_APP_MANIFEST_BYTES)?;
    let [(root_name, Value::Object(properties))] = document.as_slice() else {
        return Err(DiscoveryError::InvalidKeyValues("appstate_root"));
    };
    if !root_name.eq_ignore_ascii_case("AppState") {
        return Err(DiscoveryError::InvalidKeyValues("appstate_root"));
    }

    let app_id = text_property(properties, "appid")?;
    if app_id != "3513350" {
        return Err(DiscoveryError::InvalidKeyValues("unexpected_appid"));
    }
    let install_dir = text_property(properties, "installdir")?;
    if install_dir.is_empty()
        || install_dir.len() > 255
        || install_dir.trim() != install_dir
        || install_dir == "."
        || install_dir == ".."
        || install_dir.contains(['/', '\\', ':'])
    {
        return Err(DiscoveryError::InvalidKeyValues("unsafe_installdir"));
    }
    Ok(install_dir.to_owned())
}

fn text_property<'a>(
    properties: &'a [(String, Value)],
    expected_key: &str,
) -> Result<&'a str, DiscoveryError> {
    properties
        .iter()
        .find_map(|(key, value)| {
            key.eq_ignore_ascii_case(expected_key)
                .then_some(value)
                .and_then(|value| match value {
                    Value::Text(text) => Some(text.as_str()),
                    Value::Object(_) => None,
                })
        })
        .ok_or(DiscoveryError::InvalidKeyValues("missing_property"))
}
