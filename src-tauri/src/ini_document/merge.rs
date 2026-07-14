use super::encoding::Encoding;
use super::line::{preferred_terminator, render, trim_ascii, Line};
use super::IniError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedChange {
    section: String,
    key: String,
    value: Option<String>,
}

impl ManagedChange {
    pub fn set(
        section: impl Into<String>,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        Self {
            section: section.into(),
            key: key.into(),
            value: Some(value.into()),
        }
    }

    pub fn delete(section: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            section: section.into(),
            key: key.into(),
            value: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticChange {
    pub section: String,
    pub key: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergePreview {
    pub before: Vec<u8>,
    pub after: Vec<u8>,
    pub semantic_changes: Vec<SemanticChange>,
}

pub(crate) fn merge(
    original: &[u8],
    encoding: Encoding,
    source_lines: &[Line],
    changes: &[ManagedChange],
) -> Result<MergePreview, IniError> {
    let mut lines = source_lines.to_vec();
    let mut semantic_changes = Vec::new();

    for change in changes {
        let matches = matching_keys(&lines, &change.section, &change.key);
        if matches.len() > 1 {
            return Err(IniError::AmbiguousManagedKey {
                section: change.section.clone(),
                key: change.key.clone(),
            });
        }

        match (matches.first().copied(), change.value.as_deref()) {
            (Some(index), Some(value)) => {
                let before = lines[index]
                    .value()
                    .expect("managed key lines contain a value")
                    .to_owned();
                if before != value {
                    lines[index].replace_value(value);
                    semantic_changes.push(semantic_change(change, Some(before), Some(value)));
                }
            }
            (Some(index), None) => {
                let before = lines[index]
                    .value()
                    .expect("managed key lines contain a value")
                    .to_owned();
                lines.remove(index);
                semantic_changes.push(semantic_change(change, Some(before), None));
            }
            (None, Some(value)) => {
                insert_value(&mut lines, &change.section, &change.key, value);
                semantic_changes.push(semantic_change(change, None, Some(value)));
            }
            (None, None) => {}
        }
    }

    let after = encoding.encode(&render(&lines));
    Ok(MergePreview {
        before: original.to_vec(),
        after,
        semantic_changes,
    })
}

fn semantic_change(
    change: &ManagedChange,
    before: Option<String>,
    after: Option<&str>,
) -> SemanticChange {
    SemanticChange {
        section: change.section.clone(),
        key: change.key.clone(),
        before,
        after: after.map(str::to_owned),
    }
}

fn matching_keys(lines: &[Line], section: &str, key: &str) -> Vec<usize> {
    let section = trim_ascii(section);
    let key = trim_ascii(key);
    let mut in_section = false;
    let mut matches = Vec::new();

    for (index, line) in lines.iter().enumerate() {
        if let Some(name) = line.section_name() {
            in_section = name.eq_ignore_ascii_case(section);
        } else if in_section
            && line
                .key_name()
                .is_some_and(|name| name.eq_ignore_ascii_case(key))
        {
            matches.push(index);
        }
    }
    matches
}

fn insert_value(lines: &mut Vec<Line>, section: &str, key: &str, value: &str) {
    let section = trim_ascii(section);
    let key = trim_ascii(key);
    let terminator = preferred_terminator(lines).to_owned();
    let insertion = section_insertion_index(lines, section);
    let new_key = format!("{key}={value}");

    match insertion {
        Some(index) if index < lines.len() => {
            lines.insert(index, Line::new(new_key, &terminator));
        }
        Some(_) => append_lines(lines, vec![new_key], &terminator),
        None => append_lines(lines, vec![format!("[{section}]"), new_key], &terminator),
    }
}

fn section_insertion_index(lines: &[Line], section: &str) -> Option<usize> {
    let section = trim_ascii(section);
    let mut in_target = false;
    let mut insertion = None;

    for (index, line) in lines.iter().enumerate() {
        if let Some(name) = line.section_name() {
            if in_target {
                insertion = Some(index);
            }
            in_target = name.eq_ignore_ascii_case(section);
        }
    }
    if in_target {
        insertion = Some(lines.len());
    }
    insertion
}

fn append_lines(lines: &mut Vec<Line>, texts: Vec<String>, terminator: &str) {
    let had_lines = !lines.is_empty();
    let ended_with_terminator = lines.last().is_some_and(|line| !line.terminator.is_empty());

    if had_lines && !ended_with_terminator {
        lines.last_mut().expect("line exists").terminator = terminator.to_owned();
    }

    let text_count = texts.len();
    for (index, text) in texts.into_iter().enumerate() {
        let is_last = index + 1 == text_count;
        let added_terminator = if is_last && had_lines && !ended_with_terminator {
            ""
        } else {
            terminator
        };
        lines.push(Line::new(text, added_terminator));
    }
}
