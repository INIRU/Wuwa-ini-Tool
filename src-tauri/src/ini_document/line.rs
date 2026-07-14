#[derive(Clone, Debug)]
pub(crate) struct Line {
    pub(crate) text: String,
    pub(crate) terminator: String,
}

impl Line {
    pub(crate) fn new(text: String, terminator: &str) -> Self {
        Self {
            text,
            terminator: terminator.to_owned(),
        }
    }

    pub(crate) fn parse_all(text: &str) -> Vec<Self> {
        let bytes = text.as_bytes();
        let mut lines = Vec::new();
        let mut start = 0;
        let mut index = 0;

        while index < bytes.len() {
            let terminator_length = match bytes[index] {
                b'\r' if bytes.get(index + 1) == Some(&b'\n') => 2,
                b'\r' | b'\n' => 1,
                _ => {
                    index += 1;
                    continue;
                }
            };
            lines.push(Self::new(
                text[start..index].to_owned(),
                &text[index..index + terminator_length],
            ));
            index += terminator_length;
            start = index;
        }

        if start < text.len() {
            lines.push(Self::new(text[start..].to_owned(), ""));
        }
        lines
    }

    pub(crate) fn section_name(&self) -> Option<&str> {
        let trimmed = trim_ascii(&self.text);
        let inner = trimmed.strip_prefix('[')?.strip_suffix(']')?;
        let name = trim_ascii(inner);
        (!name.is_empty()).then_some(name)
    }

    pub(crate) fn key_name(&self) -> Option<&str> {
        let trimmed = trim_ascii(&self.text);
        if trimmed.starts_with(';') || trimmed.starts_with('#') {
            return None;
        }

        let equals = self.text.find('=')?;
        let key = trim_ascii(&self.text[..equals]);
        (!key.is_empty()).then_some(key)
    }

    pub(crate) fn value(&self) -> Option<&str> {
        let layout = ValueLayout::from_text(&self.text)?;
        Some(&self.text[layout.value_start..layout.value_end])
    }

    pub(crate) fn replace_value(&mut self, value: &str) {
        let layout = ValueLayout::from_text(&self.text).expect("managed key lines contain equals");
        self.text
            .replace_range(layout.value_start..layout.value_end, value);
    }
}

pub(crate) fn render(lines: &[Line]) -> String {
    let capacity = lines
        .iter()
        .map(|line| line.text.len() + line.terminator.len())
        .sum();
    let mut text = String::with_capacity(capacity);
    for line in lines {
        text.push_str(&line.text);
        text.push_str(&line.terminator);
    }
    text
}

pub(crate) fn preferred_terminator(lines: &[Line]) -> &str {
    lines
        .iter()
        .find_map(|line| (!line.terminator.is_empty()).then_some(line.terminator.as_str()))
        .unwrap_or("\n")
}

fn trim_ascii(value: &str) -> &str {
    value.trim_matches(|character: char| character.is_ascii_whitespace())
}

#[derive(Debug)]
struct ValueLayout {
    value_start: usize,
    value_end: usize,
}

impl ValueLayout {
    fn from_text(text: &str) -> Option<Self> {
        let equals = text.find('=')?;
        let rest_start = equals + 1;
        let rest = &text[rest_start..];
        let comment = rest
            .bytes()
            .enumerate()
            .find(|(index, byte)| {
                (*byte == b';' || *byte == b'#')
                    && (*index == 0 || rest.as_bytes()[*index - 1].is_ascii_whitespace())
            })
            .map(|(index, _)| index);

        let suffix_start = match comment {
            Some(comment_start) => rest.as_bytes()[..comment_start]
                .iter()
                .rposition(|byte| !byte.is_ascii_whitespace())
                .map_or(0, |index| index + 1),
            None => rest
                .bytes()
                .rposition(|byte| !byte.is_ascii_whitespace())
                .map_or(0, |index| index + 1),
        };
        let leading = rest.as_bytes()[..suffix_start]
            .iter()
            .take_while(|byte| byte.is_ascii_whitespace())
            .count();

        Some(Self {
            value_start: rest_start + leading,
            value_end: rest_start + suffix_start,
        })
    }
}
